//! Rule-driven topic scanner shared by the queue extractor.
//!
//! The queue extractor used to read a topic with two hard-coded assumptions:
//!
//!   1. `source.find(needle)` — the FIRST occurrence in the file. A file that
//!      publishes to `orders` and to `payments` produced one node and whichever
//!      topic lost was invisible to the graph.
//!   2. the literal had to sit immediately after an optional `(`. Every object /
//!      kwarg / struct / named-argument form in every language
//!      (`send({topic:'x'})`, `basic_publish(routing_key='x')`,
//!      `kafka.Message{Topic:"x"}`, `Arrays.asList("x")`, `['topic' => 'x']`)
//!      read as "no topic" and collapsed to the framework tag.
//!
//! This module replaces both. [`scan`] walks EVERY occurrence of a needle (up to
//! [`MAX_HITS_PER_NEEDLE`]) and reads the topic out of the call's argument region
//! according to a per-needle [`TopicRule`], so the pattern table says how a topic
//! is spelled instead of the scanner guessing.
//!
//! Precision is deliberate: [`TopicRule::ArgLiteral`] looks inside positional
//! argument #0 ONLY, so `producer.send(topicVar, "payload")` still yields no
//! topic rather than mistaking the payload for one.
//!
//! No panics: every slice goes through `get()` or [`clip`], because a slicing
//! panic here happens inside the engine's `catch_unwind` and silently drops a
//! whole file's parse.

use crate::code_guard::LazyGuard;

/// One occurrence of a needle in a source file.
pub struct TopicHit {
    /// The topic this occurrence names, if the rule could read one.
    pub topic: Option<String>,
    /// A2.6: the literal shape `topic` was folded from. `Literal` when `topic`
    /// is `None`, and always for the identity rules.
    pub form: TopicForm,
    /// Byte index of the needle in the source (stable, for ordering/debug).
    pub offset: usize,
    /// LA.4 (A11.7): the identifier expression sitting in the rule's topic
    /// slot when no literal was read there (`ORDERS_TOPIC`, `Topics.ORDERS`,
    /// `this.topic`, the key of an ES6 shorthand `{ topic }`). `None` whenever
    /// `topic` is `Some`, for `NoIdentity` and the A2.5 identity rules, and
    /// for anything that is not a plain identifier chain. Recorded, never
    /// resolved here: the engine folds it through the repo const table after
    /// the parse cache (`queues::extract_queue_nodes_with_consts`).
    pub expr: Option<String>,
    /// DT.2c: a [`TopicRule::NatsSubject`] / [`TopicRule::NatsFilter`] slot
    /// that held a shape no rule could read: a token only partly dynamic
    /// (`"orders-" + region`), a call that is no one-return subject function,
    /// an interpolated filter. Never set beside `topic` or `expr`, and never
    /// for any other rule. The extractor counts it as an expression, so CL.1
    /// rule (ii) never explains a dynamic NATS call away.
    pub dynamic: bool,
    /// DT.2c: `expr` is a `fmt.Sprintf` FORMAT (`fmt.Sprintf(RefundSubject,
    /// n)`), so the LA.4 fold reads its value through
    /// [`subject_from_format`], never as the topic itself.
    pub format: bool,
    /// DT.2c: how a NATS publish subject was folded to its `*`-token form
    /// (`topic` is then that form), for the `[queues] subject-fold` marker.
    /// `None` for a literal and for every other rule.
    pub fold: Option<SubjectFold>,
}

/// A2.6: the shape a topic literal had before [`fold_topic`] reduced it to a
/// name. Cloud brokers name a queue by URL, ARN or resource path, and both
/// sides of a flow only join if every one of those folds to the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopicForm {
    /// Taken verbatim (every file-local broker, every identity rule).
    Literal,
    /// `arn:aws:sns:us-east-1:123456789012:orders` -> `orders`.
    Arn,
    /// `https://sqs.us-east-1.amazonaws.com/123456789012/orders` -> `orders`.
    Url,
    /// `projects/my-project/topics/orders` -> `orders`.
    Path,
}

impl TopicForm {
    /// The `from=` token of the `[queues] cloud broker=` marker.
    pub fn as_str(self) -> &'static str {
        match self {
            TopicForm::Literal => "literal",
            TopicForm::Arn => "arn",
            TopicForm::Url => "url",
            TopicForm::Path => "path",
        }
    }
}

/// A topic name plus the shape it was folded from.
type Folded = (String, TopicForm);

/// How a topic is spelled at a given needle.
#[derive(Clone, Copy, Debug)]
pub enum TopicRule {
    /// First quoted literal inside positional argument #0.
    ArgLiteral,
    /// First quoted literal inside positional argument N (AMQP
    /// `basic_publish(exchange, routing_key)` and friends).
    ArgIndex(usize),
    /// CL.1: positional argument N, falling back to argument M only when
    /// argument N is present and is an EMPTY literal (`''`, `""`, ``` `` ```).
    /// amqplib `publish(exchange, routingKey, content)`: the routing key names
    /// the queue on the default and direct exchanges, and a fanout publish
    /// (`publish('logs', '', buf)`) passes an empty key, so the exchange is
    /// the only identity left.
    ArgIndexOr(usize, usize),
    /// `topic: 'x'` / `Topic: "x"` / `topics = "x"` / `'topic' => 'x'` /
    /// `queue: :x`. Keys are tried in table order, first match wins.
    Keyed(&'static [&'static str]),
    /// Keyed form first, falling back to argument #0 (APIs that accept both).
    KeyedOrArg(&'static [&'static str]),
    /// CL.3: builder-method keys read in the fluent chain that FOLLOWS the
    /// needle, up to the end of its statement ([`chain_region`]): Azure's
    /// `new ServiceBusClientBuilder().sender().queueName("orders")`, the AWS
    /// SDK v2 `ReceiveMessageRequest.builder().queueUrl("..")` built on its
    /// own statement, MQTTnet's `new MqttApplicationMessageBuilder()
    /// .WithTopic("x")`. Keys are tried in table order, first match wins, and
    /// each is read as a builder method (`.key("x")`), the A2.6 form of
    /// [`TopicRule::Keyed`].
    Chain(&'static [&'static str]),
    /// The needle proves the framework but never names a topic.
    NoIdentity,
    // --- A2.5: TASK-QUEUE IDENTITY. These read a SYMBOL, not a literal, ------
    // because for Celery/Sidekiq/Dramatiq/Oban the join key is the task and the
    // first string argument is a payload. See the block comment in `queues.rs`.
    /// The symbol chain immediately BEFORE the needle, last segment:
    /// `tasks.send_email.delay(` -> `send_email`, `HardWorker.perform_async(`
    /// -> `HardWorker`.
    Receiver,
    /// The callee named by positional argument #0, stepping over a constructor
    /// segment: `Oban.insert(EmailWorker.new(%{}))` -> `EmailWorker`.
    ArgReceiver,
    /// The first def-like identifier within [`LOOKAHEAD_LINES`] after the
    /// needle's line — `@shared_task` over `def send_email` -> `send_email`.
    /// Keeps scanning, so a stack of decorators does not hide the definition.
    DeclaredSymbol,
    /// The nearest `class` / `module` / `defmodule` / `struct` above the needle,
    /// within [`ENCLOSING_LOOKBACK_LINES`] — `include Sidekiq::Worker` inside
    /// `class HardWorker` -> `HardWorker`.
    EnclosingSymbol,
    /// CL.4: the method a job LAMBDA calls, for task queues whose job is a
    /// method (JobRunr, Hangfire): `BackgroundJob.enqueue(() ->
    /// emailJobs.sendWelcome(id))` -> `sendWelcome`, `Enqueue<T>(x =>
    /// x.SendWelcome(id))` -> `SendWelcome`. See [`lambda_callee`]. A call
    /// with no lambda (`queue.Enqueue(item)`, a method reference) reads
    /// nothing, and the extractor treats that hit as no site at all.
    LambdaCallee,
    // --- DG.1: TYPED-JOB IDENTITY. apalis (Rust) names a job by its TYPE. ----
    /// DG.1: an apalis storage push, `storage.push(Email { .. })`. The
    /// receiver must be declared in the file as a storage ([`storage_job_type`]);
    /// the job is the storage's type argument, else the pushed value's type.
    /// `.push(` is every `Vec`'s method, so an occurrence the rule cannot
    /// prove is no site at all ([`scan_guarded`] drops it).
    StorageJobType,
    /// DG.1: an apalis worker, `WorkerBuilder::new(..).backend(s)
    /// .build_fn(send_email)`. The statement must hold `WorkerBuilder`
    /// ([`worker_job_type`]); the job is the handler's first parameter type,
    /// else the backend storage's type argument. `.build(` is every builder's
    /// method, so an occurrence the rule cannot prove is no site at all.
    WorkerJobType,
    // --- DH.1: CALLS THAT NAME A QUEUE ELSEWHERE, OR WITHOUT PARENTHESES ----
    /// DH.1: the call names no queue; the file's one DECLARED queue is its
    /// identity. Bunny's `q = ch.queue("orders")` + `q.subscribe(block: true)
    /// { .. }`: every occurrence reads no topic and no expression, so the hit
    /// exists only for CL.1's declare pass (`queues::bind_declared_queues`) to
    /// bind, and an unbound one keeps the deferred framework tag.
    Declared,
    /// DH.1: the first argument of a call written with or without parentheses,
    /// a quoted literal or a Ruby / Elixir `:symbol` ([`bare_arg`]): Karafka's
    /// routing DSL `topic :orders do .. end`, google-cloud-pubsub's
    /// `pubsub.topic "orders"`. A bare-word needle (`topic `) reads only at a
    /// word edge ([`bare_word_ok`]), and an occurrence that reads no literal is
    /// no site at all ([`scan_guarded`] drops it), so `topic = x`, a prose
    /// `topic name` and `topic Topics::ORDERS` mint nothing. Never an
    /// expression: the LA.4 fold has no slot to read here.
    BareArg,
    // --- DG.2: A CONSTRUCTOR WHOSE FIRST ARGUMENT IS THE TOPIC ---------------
    /// DG.2: a dotless TYPE needle read only where it is CONSTRUCTED, arg #0's
    /// literal ([`ctor_region`]): Kafka's `ProducerRecord(topic, ..)` in every
    /// JVM spelling, Java `new ProducerRecord<String, String>("orders", v)`
    /// and its diamond `<>`, Scala `new ProducerRecord[String, String](
    /// "orders", v)`, Kotlin `ProducerRecord<String, String>("orders", v)`
    /// with no `new`. One optional type-argument list (`<..>` or `[..]`) is
    /// stepped over, then a `(` must follow: a declaration
    /// (`ProducerRecord<K, V> r = ..`), an import line and `classOf[
    /// ProducerRecord[..]]` are no site at all ([`scan_guarded`] drops them).
    /// A non-literal arg #0 goes to the LA.4 fold. The needle reads only at a
    /// word edge ([`bare_word_ok`]), so `MyProducerRecord(` and
    /// `ProducerRecords` are not it.
    CtorArgLiteral,
    // --- DH.3: A COMMAND VECTOR WHOSE VERB IS A LITERAL ---------------------
    /// DH.3: a raw command vector in the argument region, the quoted literal
    /// right after the element equal (ASCII case-insensitive) to the command
    /// ([`command_arg`]): package:redis's `cmd.send_object(['PUBLISH',
    /// 'orders', payload])`. The command must OPEN its vector (the region's
    /// first argument, or the first element of a list inside it), the way a
    /// Redis command always does, so `['SET', 'publish', v]` is no publish. A
    /// call whose vector names another command (`['SET', k, v]`) is no site at
    /// all ([`scan_guarded`] drops it); the command with no literal after it
    /// (`['PUBLISH', channel, m]`) is a site that reads no topic, so it keeps
    /// the framework tag. Never an expression: the LA.4 fold has no slot here.
    CommandArg(&'static str),
    // --- DH.4: A KEYWORD OPTION INSIDE THE ENCLOSING TUPLE ------------------
    /// DH.4: `<key>: <value>` read in the bracket the needle opens, else in the
    /// innermost bracket group ENCLOSING the needle ([`enclosing_region`]):
    /// Elixir's Broadway producer tuple `{BroadwayKafka.Producer, hosts: [..],
    /// topics: ["orders"]}`, often spread over several lines, and Tortoise's
    /// `Tortoise311.Supervisor.start_child(.., subscriptions: [{"sensors/temp",
    /// 0}])`. Keys are tried in table order with [`TopicRule::Keyed`]'s key
    /// search (word edges; `:` / `=>` / `=` separators); the value is a
    /// literal, or the first literal inside up to two opening `[` / `{`
    /// ([`keyed_in`]). No region, or a region where no key is followed by a
    /// separator, is no site at all ([`scan_guarded`] drops it): an `alias
    /// BroadwayKafka.Producer` line, a `{Gnat.ConsumerSupervisor, settings}`
    /// tuple and a Tortoise connection that subscribes to nothing mint no
    /// consumer. A key whose value is no literal (`topics: @topics`) is a site
    /// that reads no topic, so it keeps the framework tag. Never an
    /// expression: an option value held in a variable names nothing the LA.4
    /// fold has a slot for.
    KeyedIn(&'static [&'static str]),
    // --- DT.2b: nats.go / JetStream, A SUBJECT THAT MAY SIT BEHIND A ctx -----
    /// DT.2b: a NATS PUBLISH subject: positional argument #0, or argument #1
    /// when argument #0 is a context expression ([`nats_subject_slot`]: the
    /// new jetstream API's `js.Publish(ctx, "orders.new", d)`). DT.2c: the slot
    /// reads on the subject grammar ([`nats_subject`]): a literal is the
    /// subject; a concatenation, an interpolated literal, a `fmt.Sprintf`
    /// with a literal format or a same-file one-return subject function folds
    /// each WHOLE dynamic token to `*` (`"shop.shipments." + id` ->
    /// `shop.shipments.*`); an identifier chain, or a `fmt.Sprintf` whose
    /// format is one, is the LA.4 fold's expression; anything else, a token
    /// only partly dynamic included, is dynamic ([`TopicHit::dynamic`]).
    /// The nats.go rows and the HEAD `nc.publish` / `nc.Publish` rows.
    NatsSubject,
    /// DT.2b: a NATS SUBSCRIBE filter (`conn.Subscribe("orders.*", cb)`,
    /// `js.PullSubscribe(subj, durable)`), in the slot
    /// [`TopicRule::NatsSubject`] reads. DT.2c ([`nats_filter`]): a literal
    /// or an identifier chain reads as before; a concatenation,
    /// interpolation, format or call is dynamic, never folded, because a
    /// filter's `*` is a real wildcard and a runtime token is not one.
    NatsFilter,
}

/// Bound on occurrences examined per needle per file — generated files can
/// contain thousands of identical calls and every hit costs a region walk.
pub const MAX_HITS_PER_NEEDLE: usize = 32;

/// Bound on the argument region walked for one occurrence.
pub const MAX_REGION: usize = 2048;

/// Longest topic accepted. Anything longer is a payload, not an identifier.
pub const MAX_TOPIC_LEN: usize = 128;

/// Lines examined after a decorator for the definition it decorates.
pub const LOOKAHEAD_LINES: usize = 5;

/// Lines walked back looking for the enclosing class/module. Bounded because
/// this runs once per needle occurrence and a generated file can be huge.
pub const ENCLOSING_LOOKBACK_LINES: usize = 200;

/// Def-like keywords, tried IN THIS ORDER, so `public class Foo` reads as a
/// class and not as a `public` whose next word is `class`. CL.4: Kotlin's
/// `fun` (a JobRunr `@Job` method) comes last, so it only ever reads a line
/// no other keyword claims.
const DEF_KEYWORDS: &[&str] = &[
    "def ",
    "fn ",
    "func ",
    "function ",
    "class ",
    "void ",
    "public ",
    "fun ",
];

/// CL.4: keywords that declare a TYPE, read before a declared line's `(`
/// ([`declares_type`]). `object` is Scala's; a C# `object` parameter sits
/// after the `(`, so it never counts.
const TYPE_KEYWORDS: &[&str] = &[
    "class ",
    "struct ",
    "interface ",
    "record ",
    "enum ",
    "object ",
    "trait ",
];

/// Scope-introducing keywords for [`TopicRule::EnclosingSymbol`], longest-first
/// so `defmodule` is never read as `module`.
const SCOPE_KEYWORDS: &[&str] = &["defmodule ", "class ", "module ", "struct "];

/// Segments that are never a task identity — language keywords and the
/// constructor segment of `Worker.new(...)`.
const NOT_A_SYMBOL: &[&str] = &[
    "return", "await", "self", "this", "new", "end", "do", "yield", "async", "let", "const", "var",
];

/// Truncate `s` to at most `n` bytes, walking back to a char boundary.
/// (`str::floor_char_boundary` is still unstable; raw slicing would panic.)
pub fn clip(s: &str, n: usize) -> &str {
    if n >= s.len() {
        return s;
    }
    let mut i = n;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    s.get(..i).unwrap_or("")
}

/// Every occurrence of `needle` in `source`, each with the topic its argument
/// region names under `rule` (or `None` when the rule cannot read one).
/// No literal / comment guard: [`scan_guarded`] with a file that has none.
pub fn scan(source: &str, needle: &str, rule: TopicRule) -> Vec<TopicHit> {
    scan_guarded(source, needle, rule, &mut LazyGuard::new("", source))
}

/// CJ.1a: [`scan`] over the occurrences that may be call sites. An
/// occurrence is dropped, IN THIS ORDER, when it is a bare-word needle inside
/// a longer identifier ([`bare_word_ok`]) or when `guard` refuses it (its
/// first byte sits in a Rust / Python string literal or comment), and only
/// then is [`MAX_HITS_PER_NEEDLE`] applied, so a needle table's literal
/// occurrences can no longer crowd a real call out of the cap.
///
/// DG.1: a [`TopicRule::StorageJobType`] / [`TopicRule::WorkerJobType`]
/// occurrence that reads no job type is dropped at the same point, before the
/// cap: the rule's proof (a storage receiver, a `WorkerBuilder` statement) IS
/// the site, so an unproven `.push(` / `.build(` is no call of the framework,
/// becomes no hit, and so records no `unresolved:` tag downstream.
///
/// DH.1: a [`TopicRule::BareArg`] occurrence that reads no literal is dropped
/// at the same point, for the same reason: the literal IS the site, and a
/// `topic` word that names nothing (`topic = x`, `topic Topics::ORDERS`) is
/// no call of the framework. A [`TopicRule::Declared`] occurrence is always a hit, with
/// no topic and no expression.
///
/// DG.2: a [`TopicRule::CtorArgLiteral`] occurrence that constructs nothing
/// ([`ctor_region`] finds no `(`) is dropped at the same point: a type named in
/// an import or a declaration is no call. A constructor whose arg #0 is no
/// literal stays a hit, with its expression for the LA.4 fold.
///
/// DH.3: a [`TopicRule::CommandArg`] occurrence whose argument region holds no
/// vector opened by the command is dropped at the same point (the command IS
/// the site: a `send_object(['SET', k, v])` is no publish). One whose command
/// has no literal after it stays a hit, with no topic and no expression.
///
/// DH.4: a [`TopicRule::KeyedIn`] occurrence with no region, or whose region
/// holds none of its keys followed by a separator, is dropped at the same
/// point (the option IS the site). One whose key holds no literal stays a
/// hit, with no topic and no expression.
pub(crate) fn scan_guarded(
    source: &str,
    needle: &str,
    rule: TopicRule,
    guard: &mut LazyGuard<'_>,
) -> Vec<TopicHit> {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    for (offset, _) in source.match_indices(needle) {
        if hits.len() >= MAX_HITS_PER_NEEDLE {
            break;
        }
        if !bare_word_ok(source, offset, needle, rule) || !guard.admits(offset) {
            continue;
        }
        let after = offset.saturating_add(needle.len());
        // DT.2c: a NATS subject or filter reads on the subject grammar.
        if matches!(rule, TopicRule::NatsSubject | TopicRule::NatsFilter) {
            hits.push(nats_hit(source, after, needle, rule, offset));
            continue;
        }
        let read = match rule {
            // DH.1: a Declared call names its queue elsewhere (the file's
            // declaration), so it reads nothing here.
            TopicRule::NoIdentity | TopicRule::Declared => None,
            // DH.1: the literal, or no site at all (see the doc above).
            TopicRule::BareArg => match bare_arg(source, after) {
                Some(read) => Some(read),
                None => continue,
            },
            // A2.5: identity rules read the code AROUND the call, not its
            // arguments, so they need the needle's own offset. A symbol is
            // never a URL/ARN, so its form is always `Literal`.
            TopicRule::Receiver => verbatim(receiver_before(source, offset)),
            TopicRule::ArgReceiver => verbatim(arg_receiver(source, offset, after, needle)),
            TopicRule::DeclaredSymbol => verbatim(declared_symbol(source, after)),
            TopicRule::EnclosingSymbol => verbatim(enclosing_symbol(source, offset)),
            TopicRule::LambdaCallee => verbatim(lambda_callee(source, after, needle)),
            // DG.1: the job TYPE, or no site at all (see the doc above).
            TopicRule::StorageJobType => match storage_job_type(source, offset, after, needle) {
                Some(job) => verbatim(Some(job)),
                None => continue,
            },
            TopicRule::WorkerJobType => match worker_job_type(source, offset, after, needle) {
                Some(job) => verbatim(Some(job)),
                None => continue,
            },
            // DG.2: a mention that constructs nothing (a declaration, an
            // import) is no site at all (see the variant's doc).
            TopicRule::CtorArgLiteral if ctor_region(source, after).is_none() => continue,
            // DH.3: the literal after the command, or no site at all when
            // no vector opens with the command (see the doc above).
            TopicRule::CommandArg(cmd) => {
                match arg_region(source, after, needle).and_then(|r| command_arg(r, cmd)) {
                    Some(read) => read,
                    None => continue,
                }
            }
            // DH.4: the keyed option, or no site at all when no region holds
            // one of its keys (see the doc above).
            TopicRule::KeyedIn(keys) => {
                let region = arg_region(source, after, needle)
                    .or_else(|| enclosing_region(source, offset));
                match region.and_then(|r| keyed_in(r, keys)) {
                    Some(read) => read,
                    None => continue,
                }
            }
            _ => topic_at(source, after, needle, rule),
        };
        let (topic, form, expr) = match read {
            Some((t, f)) => (Some(t), f, None),
            None => (
                None,
                TopicForm::Literal,
                expr_at(source, after, needle, rule),
            ),
        };
        hits.push(TopicHit {
            topic,
            form,
            offset,
            expr,
            dynamic: false,
            format: false,
            fold: None,
        });
    }
    hits
}

fn verbatim(symbol: Option<String>) -> Option<Folded> {
    symbol.map(|s| (s, TopicForm::Literal))
}

/// Trim quoting/punctuation noise off a raw literal, fold a cloud identity to
/// its name, and reject non-identifiers. See [`fold_topic`].
pub fn normalise_topic(raw: &str) -> Option<String> {
    fold_topic(raw).map(|(t, _)| t)
}

/// [`normalise_topic`], also reporting which shape the literal had.
///
/// A2.6 — the fold runs BEFORE the length/newline gates, so a 140-byte queue
/// URL whose name is `orders` is accepted as `orders`. Branches, in order:
///
/// * ARN: `arn:aws:sqs:us-east-1:123456789012:orders` -> `orders`. An ARN has
///   at least 6 colon-separated fields (`arn:partition:service:region:account:
///   resource`) and the queue/topic name is the LAST one. Fewer fields is not
///   an ARN and is kept verbatim.
/// * URL: `https://sqs.us-east-1.amazonaws.com/123456789012/orders` -> `orders`
///   (also `http://localhost:4566/000000000000/orders`, localstack). The last
///   non-empty path segment, after dropping `?query` and `#fragment`. A URL
///   with no path names no queue, so it is rejected, not kept.
/// * GCP path: `projects/my-project/topics/orders` -> `orders`, and
///   `projects/my-project/subscriptions/orders-worker` -> `orders-worker`.
///   Exactly that 4-segment shape; any other `projects/...` string (an MQTT
///   topic, say) is kept verbatim.
///
/// A folded segment that is still a placeholder (`.../${QUEUE}`,
/// `f".../{name}"`) is rejected: it names a variable, not a queue, so the
/// caller falls back to the unresolved sentinel instead of minting `${QUEUE}`.
pub fn fold_topic(raw: &str) -> Option<Folded> {
    // Trimmed ONCE: a `Literal` must come out byte-identical to the pre-A2.6
    // `normalise_topic`, so it is never trimmed a second time.
    let (t, form) = fold_cloud_identity(trim_noise(raw))?;
    if t.is_empty() || t.len() > MAX_TOPIC_LEN || t.contains('\n') || t.contains('\r') {
        return None;
    }
    Some((t.to_string(), form))
}

fn trim_noise(raw: &str) -> &str {
    raw.trim()
        .trim_matches(|c: char| matches!(c, ',' | ')' | ']' | '}' | '\'' | '"' | '`'))
        .trim()
}

/// The branch table of [`fold_topic`]. `None` = a recognised cloud shape that
/// carries no usable name.
fn fold_cloud_identity(t: &str) -> Option<(&str, TopicForm)> {
    if t.starts_with("arn:") && t.split(':').count() >= 6 {
        return folded_segment(t.rsplit(':').next()?, TopicForm::Arn);
    }
    if let Some(rest) = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
    {
        let path = rest.split(['?', '#']).next()?;
        let mut segs = path.split('/').filter(|s| !s.is_empty());
        segs.next()?; // the host
        return folded_segment(segs.next_back()?, TopicForm::Url);
    }
    // Only the exact GCP resource shape: an MQTT topic is also `/`-separated,
    // and `projects/acme/sensors/temp` must not collapse to `temp`.
    if let Some(rest) = t.strip_prefix("projects/") {
        let segs: Vec<&str> = rest.split('/').collect();
        if let [_project, "topics" | "subscriptions", name] = segs.as_slice() {
            return folded_segment(name, TopicForm::Path);
        }
    }
    Some((t, TopicForm::Literal))
}

fn folded_segment(seg: &str, form: TopicForm) -> Option<(&str, TopicForm)> {
    let seg = seg.trim();
    if seg.contains(['{', '}', '$']) {
        return None;
    }
    Some((seg, form))
}

/// 0-indexed line number containing byte `offset`.
///
/// 0-indexed is the tree-sitter convention every other span in the graph uses
/// (CODE_RULES §4, `parsers/code/docs/src/lib.rs`), so a queue POSITION cell is
/// directly comparable with a FUNCTION's. `offset` always comes from
/// `str::match_indices`, so `source[..offset]` is on a char boundary; the
/// `get()` keeps it panic-free anyway, because a panic here happens inside the
/// engine's `catch_unwind` and silently drops the whole file's parse.
pub fn line_of(source: &str, offset: usize) -> usize {
    source
        .get(..offset.min(source.len()))
        .unwrap_or("")
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
}

/// Minimal JSON string escaping for a cell payload built by `format!`.
///
/// Same shape as `cron.rs`'s private `escape_json`; the two are deliberately
/// NOT shared yet because unifying them means editing `cron.rs`, which belongs
/// to another packet's file set this wave. See the `followups` note: one
/// `glia_code_domain` helper should replace both.
///
/// Backslash FIRST, then the quote — reversing the order would double-escape
/// the backslash it just inserted. Windows paths (`src\\a.ts`) are exactly why
/// the backslash case matters for a POSITION payload.
pub fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

fn topic_at(source: &str, after: usize, needle: &str, rule: TopicRule) -> Option<Folded> {
    // CL.3: a chain row reads no argument region, so it never walks one.
    if let TopicRule::Chain(keys) = rule {
        return keyed_literal(chain_region(source, after), keys);
    }
    // DG.2: the constructor's own argument list, past its type arguments.
    if let TopicRule::CtorArgLiteral = rule {
        return arg_literal(ctor_region(source, after)?, 0);
    }
    let region = arg_region(source, after, needle);
    match rule {
        TopicRule::ArgLiteral => arg_literal(region?, 0),
        TopicRule::ArgIndex(n) => arg_literal(region?, n),
        TopicRule::ArgIndexOr(n, m) => {
            let region = region?;
            arg_literal(region, if empty_literal_arg(region, n) { m } else { n })
        }
        // A2.5: Ruby and Elixir call the same APIs WITHOUT parentheses
        // (`sidekiq_options queue: 'critical'`), so a keyed rule that found no
        // bracketed region reads the rest of the LINE instead. Only the keyed
        // rules get this fallback: a positional rule turned loose on a bare line
        // would read the next string literal on it as argument #0.
        TopicRule::Keyed(keys) => {
            keyed_literal(region.unwrap_or_else(|| line_region(source, after)), keys)
        }
        TopicRule::KeyedOrArg(keys) => {
            let region = region?;
            keyed_literal(region, keys).or_else(|| arg_literal(region, 0))
        }
        // `NoIdentity`, the A2.5 identity rules, DH.1's `Declared` /
        // `BareArg`, DH.3's `CommandArg`, DH.4's `KeyedIn` and DT.2c's NATS
        // rules ([`nats_hit`]) never read an argument region here; `scan`
        // dispatches them before it gets here.
        _ => None,
    }
}

/// LA.4: the identifier expression in the slot [`topic_at`] read no literal
/// from — the same region, the same argument, the same key, tried in the same
/// order. `NoIdentity` and the A2.5 identity rules never carry one, nor do
/// DH.1's `Declared` (no slot) and `BareArg` (a literal or no site), DH.3's
/// `CommandArg` (a command vector's literal, no expression slot) and DH.4's
/// `KeyedIn` (an option value in a variable names nothing).
fn expr_at(source: &str, after: usize, needle: &str, rule: TopicRule) -> Option<String> {
    if matches!(
        rule,
        TopicRule::Declared
            | TopicRule::BareArg
            | TopicRule::CommandArg(_)
            | TopicRule::KeyedIn(_)
    ) {
        return None;
    }
    // CL.3: `.queueName(QUEUE)` hands `QUEUE` to the LA.4 const fold.
    if let TopicRule::Chain(keys) = rule {
        return keyed_expr(chain_region(source, after), keys);
    }
    // DG.2: `new ProducerRecord<>(TOPIC, v)` hands `TOPIC` to the LA.4 fold.
    if let TopicRule::CtorArgLiteral = rule {
        return arg_expr(ctor_region(source, after)?, 0);
    }
    let region = arg_region(source, after, needle);
    match rule {
        TopicRule::ArgLiteral => arg_expr(region?, 0),
        TopicRule::ArgIndex(n) => arg_expr(region?, n),
        TopicRule::ArgIndexOr(n, m) => {
            let region = region?;
            arg_expr(region, if empty_literal_arg(region, n) { m } else { n })
        }
        TopicRule::Keyed(keys) => {
            keyed_expr(region.unwrap_or_else(|| line_region(source, after)), keys)
        }
        TopicRule::KeyedOrArg(keys) => {
            let region = region?;
            keyed_expr(region, keys).or_else(|| arg_expr(region, 0))
        }
        // DT.2c: the NATS rules read their expression in [`nats_hit`].
        _ => None,
    }
}

/// DT.2b: the positional slot a [`TopicRule::NatsSubject`] /
/// [`TopicRule::NatsFilter`] reads: 1 when argument #0 is a Go context
/// expression (the new jetstream API takes `ctx` first), else 0. A context is
/// `ctx`, `context.Background()`, `context.TODO()` or `<chain>.Context()`
/// (`r.Context()`, `c.Request.Context()`); anything else, a literal or a
/// subject variable included, is the subject itself.
fn nats_subject_slot(region: &str) -> usize {
    let first = split_args(region).first().map_or("", |a| a.trim());
    usize::from(is_go_context_arg(first))
}

/// DT.2b: is `arg` (one trimmed argument) a Go context expression?
fn is_go_context_arg(arg: &str) -> bool {
    if matches!(arg, "ctx" | "context.Background()" | "context.TODO()") {
        return true;
    }
    arg.strip_suffix(".Context()").is_some_and(|chain| {
        !chain.is_empty()
            && !chain.starts_with(|c: char| c.is_ascii_digit())
            && chain
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
    })
}

/// The rest of the line starting at `after`, for paren-less calls.
fn line_region(source: &str, after: usize) -> &str {
    let rest = source.get(after..).unwrap_or("");
    let end = rest.find('\n').unwrap_or(rest.len());
    rest.get(..end).unwrap_or("")
}

/// The bracketed argument region that follows a needle, without its brackets.
///
/// Some needles already swallow their opening bracket (`.lpush(`, `new Worker(`);
/// the rest are bare names (`nc.Publish`, `producer.send`) followed by optional
/// whitespace and then `(`, `{` or `[`.
///
/// CL.3: a bare MEMBER-CALL needle (one holding a `.`) may be followed by a
/// generic method call's type arguments first, NATS.Net's
/// `nc.SubscribeAsync<string>("orders")` or Hangfire's
/// `BackgroundJob.Enqueue<Jobs>(x => ..)`: one balanced `<...>` is stepped
/// over ([`skip_type_args`]). A dotless TYPE needle is not: after `KafkaConsumer`
/// the type arguments open a declaration or a constructor
/// (`new KafkaConsumer<>(props)`), whose argument is client config, and
/// reading it would hand `props` to the LA.4 fold as a topic expression,
/// which keeps CL.1 rule (ii) from dropping the file's kafka tag.
pub(crate) fn arg_region<'a>(source: &'a str, after: usize, needle: &str) -> Option<&'a str> {
    opened_region(source, after, needle).map(|(_, region)| region)
}

/// CL.4: [`arg_region`] plus the bracket that opened it — `{` is a Kotlin
/// trailing lambda (`BackgroundJob.enqueue { svc.run() }`), which
/// [`lambda_callee`] reads as a lambda body.
fn opened_region<'a>(source: &'a str, after: usize, needle: &str) -> Option<(u8, &'a str)> {
    let rest = source.get(after..)?;
    if let Some(&open) = needle
        .as_bytes()
        .last()
        .filter(|c| matches!(c, b'(' | b'{' | b'['))
    {
        return region_body(rest).map(|region| (open, region));
    }
    let mut trimmed = rest.trim_start();
    if trimmed.starts_with('<') && needle.contains('.') {
        trimmed = skip_type_args(trimmed)?.trim_start();
    }
    let open = *trimmed
        .as_bytes()
        .first()
        .filter(|c| matches!(c, b'(' | b'{' | b'['))?;
    region_body(trimmed.get(1..)?).map(|region| (open, region))
}

/// CL.3: `s` (which opens with `<`) past its balanced type-argument list:
/// nested `<>` counted, so `>>` closes two levels, and quoted text stepped
/// over (`region_body`'s rules). `None` — no call to read — when the list
/// does not close within [`MAX_REGION`] bytes, or when a byte no type list
/// holds comes first (`;`, `(`, `)`, `{`, `}`, `=`, a newline): that `<` was a
/// comparison. Every byte that ends the walk is ASCII, so the slice lands on a
/// char boundary.
fn skip_type_args(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let limit = b.len().min(MAX_REGION);
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < limit {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        return s.get(i + 1..);
                    }
                }
                b';' | b'(' | b')' | b'{' | b'}' | b'=' | b'\n' | b'\r' => return None,
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// DG.2: the argument region of the constructor call a [`TopicRule::CtorArgLiteral`]
/// needle names, or `None` when the occurrence constructs nothing. After the
/// needle: spaces and tabs (never a newline: an import line's next line is
/// not its call), then at most one balanced type-argument list, a Java /
/// Kotlin `<..>` ([`skip_type_args`]; the diamond `<>` is an empty one) or a
/// Scala `[..]` ([`skip_scala_type_args`]), spaces and tabs again, then the
/// next byte must be `(`. `ProducerRecord<K, V> r = ..` (a declaration),
/// `import ..ProducerRecord;` and `classOf[ProducerRecord[K, V]]` read
/// `None`. The rule is its own variant because a dotless TYPE needle under
/// [`arg_region`] may NOT step over type arguments (`new KafkaConsumer<>(
/// props)` reads client config); a `ProducerRecord`'s arg #0 is always the
/// topic.
fn ctor_region(source: &str, after: usize) -> Option<&str> {
    let mut rest = source.get(after..)?.trim_start_matches([' ', '\t']);
    if rest.starts_with('<') {
        rest = skip_type_args(rest)?;
    } else if rest.starts_with('[') {
        rest = skip_scala_type_args(rest)?;
    }
    region_body(rest.trim_start_matches([' ', '\t']).strip_prefix('(')?)
}

/// DG.2: `s` (which opens with `[`) past its balanced Scala type-argument
/// list: nested `[]` counted (`[F[_], String]`), quoted text stepped over.
/// `None` when the list does not close within [`MAX_REGION`] bytes, or when a
/// byte no type list on one line holds comes first (`;`, a newline, `(`, `)`,
/// `{`, `}`): that `[` opened an index or an array, not type arguments. `=` is
/// NOT a stop, unlike [`skip_type_args`]: a Scala function type spells `=>`
/// (`[String => Int]`). Every byte that ends the walk is ASCII, so the slice
/// lands on a char boundary.
fn skip_scala_type_args(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let limit = b.len().min(MAX_REGION);
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < limit {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return s.get(i + 1..);
                    }
                }
                b';' | b'(' | b')' | b'{' | b'}' | b'\n' | b'\r' => return None,
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// CL.3: the fluent chain that FOLLOWS a needle, for [`TopicRule::Chain`]:
/// `.connectionString(c)\n    .sender().queueName("orders").buildClient()`.
/// Walks from `after` with [`region_body`]'s bracket and quote rules (a `//`
/// comment is skipped to its line end) and stops, exclusive, at the first of:
///
/// * a depth-0 `;` (Java / C# statement end) or `,` (the next argument or
///   initializer member);
/// * a closing bracket that takes the depth below 0: the chain was an
///   argument (`sqs.sendMessage(SendMessageRequest.builder()...build())`) or
///   an initializer;
/// * a depth-0 newline whose next non-blank byte is not `.` and whose
///   previous non-blank byte is not `.`: Scala / Kotlin / Go statements end at
///   a newline, and a Java / C# continuation line starts with `.`;
/// * [`MAX_REGION`] bytes, or EOF.
///
/// Never runs into the next statement, and every byte it stops at is ASCII, so
/// the slice lands on a char boundary.
fn chain_region(source: &str, after: usize) -> &str {
    let Some(rest) = source.get(after..) else {
        return "";
    };
    let b = rest.as_bytes();
    let limit = b.len().min(MAX_REGION);
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < limit {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        let stop = match c {
            b'\'' | b'"' | b'`' => {
                quote = Some(c);
                false
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                // A line comment ends at its newline, which is judged next.
                while i < limit && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'(' | b'{' | b'[' => {
                depth += 1;
                false
            }
            b')' | b'}' | b']' => {
                depth -= 1;
                depth < 0
            }
            b';' | b',' => depth == 0,
            b'\n' => depth == 0 && !chain_continues(b, i, limit),
            _ => false,
        };
        if stop {
            return rest.get(..i).unwrap_or("");
        }
        i += 1;
    }
    clip(rest, limit)
}

/// CL.3: does the chain carry on across the newline at `nl`? Yes when the next
/// code byte before `limit` (blank lines and `//` comment lines skipped) is
/// `.`, or the previous non-blank byte is.
fn chain_continues(b: &[u8], nl: usize, limit: usize) -> bool {
    let prev = b
        .get(..nl)
        .and_then(|head| head.iter().rev().find(|c| !c.is_ascii_whitespace()));
    if prev == Some(&b'.') {
        return true;
    }
    let mut i = nl + 1;
    while i < limit {
        match b.get(i) {
            Some(c) if c.is_ascii_whitespace() => i += 1,
            Some(b'/') if b.get(i + 1) == Some(&b'/') => {
                while i < limit && b.get(i) != Some(&b'\n') {
                    i += 1;
                }
            }
            next => return next == Some(&b'.'),
        }
    }
    false
}

/// Walk from just after an opening bracket (depth already 1) to its match,
/// respecting `'`/`"`/backtick quoting and `\` escapes. Stops at the matching
/// close, at [`MAX_REGION`] bytes, or at EOF.
pub(crate) fn region_body(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let limit = b.len().min(MAX_REGION);
    let mut depth = 1i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < limit {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'{' | b'[' => depth += 1,
                b')' | b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        // `c` is ASCII, so `i` is a char boundary.
                        return s.get(..i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    Some(clip(s, limit))
}

/// Split an argument region on depth-0 commas.
pub(crate) fn split_args(region: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = region.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'{' | b'[' => depth += 1,
                b')' | b'}' | b']' => depth -= 1,
                b',' if depth == 0 => {
                    if let Some(arg) = region.get(start..i) {
                        out.push(arg);
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if let Some(arg) = region.get(start..) {
        out.push(arg);
    }
    out
}

/// CL.1: is positional argument `n` present and exactly an empty literal
/// (`''`, `""` or ``` `` ```)? [`TopicRule::ArgIndexOr`]'s fallback test.
fn empty_literal_arg(region: &str, n: usize) -> bool {
    split_args(region)
        .get(n)
        .is_some_and(|a| matches!(a.trim(), "''" | "\"\"" | "``"))
}

/// First quoted literal inside positional argument `n`.
fn arg_literal(region: &str, n: usize) -> Option<Folded> {
    let arg = *split_args(region).get(n)?;
    let b = arg.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if matches!(b[i], b'\'' | b'"' | b'`') {
            return read_literal(arg, i);
        }
        i += 1;
    }
    None
}

/// `<key> <sep> <literal>` anywhere in the region, keys tried in table order.
///
/// A2.6: a key written as a builder METHOD, `.queueUrl("x")` / `.topicArn("x")`
/// (AWS SDK for Java v2), reads too — the `(` counts as the separator, but only
/// when a `.` sits right before the key, so a bare call like `topic("x")` still
/// does not.
fn keyed_literal(region: &str, keys: &[&str]) -> Option<Folded> {
    keyed_value(region, keys, 0).flatten()
}

/// DH.4 [`TopicRule::KeyedIn`] — [`keyed_literal`]'s key search, where the
/// value may sit inside up to two opening `[` / `{` (whitespace and line
/// breaks between them skipped): Broadway's `topics: ["orders"]`, Tortoise's
/// `subscriptions: [{"sensors/temp", 0}]`, Gnat's `subscription_topics:
/// [%{topic: "rpc.example"}]` (under the key `topic`). `Some(Some(t))`: a key
/// read a literal. `Some(None)`: a key followed by a separator, no literal
/// (`topics: @topics`), a site that names no topic. `None`: no key followed
/// by a separator, so the occurrence is no site.
fn keyed_in(region: &str, keys: &[&str]) -> Option<Option<Folded>> {
    keyed_value(region, keys, MAX_KEYED_OPENS)
}

/// DH.4: the opening brackets [`keyed_in`] steps into at a value: the list,
/// then the tuple or map inside it.
const MAX_KEYED_OPENS: usize = 2;

/// The keyed scan behind [`keyed_literal`] and [`keyed_in`]: `<key> <sep>
/// <literal>` anywhere in the region, keys tried in table order, every
/// occurrence of a key tried before the next key, and at the value at most
/// `opens` opening `[` / `{` stepped into. `Some(Some(t))` is the first literal
/// read; `Some(None)` means some key was followed by a separator but no value
/// read a literal; `None` means no key was. With `opens == 0` the first
/// literal is exactly what [`keyed_literal`] has always read.
fn keyed_value(region: &str, keys: &[&str], opens: usize) -> Option<Option<Folded>> {
    // ASCII-lowercasing is byte-length preserving, so indices map 1:1 back onto
    // `region` and we can read the ORIGINAL bytes at an index found in `lower`.
    let lower = region.to_ascii_lowercase();
    debug_assert_eq!(lower.len(), region.len());
    if lower.len() != region.len() {
        return None;
    }
    let b = region.as_bytes();
    let mut keyed = false;
    for key in keys {
        let k = key.to_ascii_lowercase();
        if k.is_empty() {
            continue;
        }
        let mut from = 0usize;
        while let Some(rel) = lower.get(from..).and_then(|s| s.find(&k)) {
            let start = from + rel;
            let end = start + k.len();
            from = end;
            if !word_edge(b, start.checked_sub(1)) || !word_edge(b, Some(end)) {
                continue;
            }
            let mut j = end;
            // optional closing quote of a quoted key: `'topic' => 'x'`
            if matches!(b.get(j), Some(b'\'' | b'"' | b'`')) {
                j += 1;
            }
            j = skip_ws(b, j);
            let builder = b.get(j) == Some(&b'(') && start > 0 && b.get(start - 1) == Some(&b'.');
            let Some(sep) = separator_len(b, j).or(builder.then_some(1)) else {
                continue;
            };
            keyed = true;
            j = skip_ws(b, j + sep);
            // CL.2: `QueueUrl: aws.String("..")` reads the literal inside.
            let j = step_into_helper(region, j).unwrap_or(j);
            // DH.4: `topics: ["orders"]` reads the literal inside the list.
            let j = step_into_opens(b, j, opens);
            if let Some(topic) = read_literal(region, j) {
                return Some(Some(topic));
            }
        }
    }
    keyed.then_some(None)
}

/// DH.4: `j` past at most `opens` opening `[` / `{`, the whitespace after each
/// skipped. Every byte stepped over is ASCII, so the index stays on a char
/// boundary.
fn step_into_opens(b: &[u8], mut j: usize, opens: usize) -> usize {
    for _ in 0..opens {
        if !matches!(b.get(j), Some(b'[' | b'{')) {
            break;
        }
        j = skip_ws(b, j + 1);
    }
    j
}

/// DH.4 [`TopicRule::KeyedIn`] — the innermost bracket group ENCLOSING the
/// byte at `offset`, without its brackets: the nearest bracket still open at
/// `offset` ([`unmatched_open_before`]), then forward to its match with
/// [`region_body`]'s rules (balanced, quotes skipped, at most [`MAX_REGION`]
/// bytes). `{BroadwayKafka.Producer, hosts: [localhost: 9092], topics:
/// ["orders"]}` is the tuple's body; the whole of a sibling's tuple never is.
/// `None` when no bracket is open within [`MAX_REGION`] bytes before the
/// needle (an `alias BroadwayKafka.Producer` line).
fn enclosing_region(source: &str, offset: usize) -> Option<&str> {
    let open = unmatched_open_before(source.as_bytes(), offset)?;
    // `open` indexes an ASCII bracket, so `open + 1` is a char boundary.
    region_body(source.get(open + 1..)?)
}

/// DH.4: the byte index of the nearest `(` / `[` / `{` still open at `end`,
/// walking BACK from `end` (exclusive) over at most [`MAX_REGION`] bytes: a
/// closing bracket met on the way is a group the next opener closes, and
/// `"`-quoted text is stepped over (a `"` after an odd run of `\` is inside
/// the string). Only `"` delimits on the way back: it is Elixir's string, and
/// an apostrophe in a `#` comment (`# don't`) must not swallow the walk. Byte
/// comparisons only, so `end` may sit anywhere.
fn unmatched_open_before(b: &[u8], end: usize) -> Option<usize> {
    let end = end.min(b.len());
    let floor = end.saturating_sub(MAX_REGION);
    let mut depth = 0usize;
    let mut quoted = false;
    let mut i = end;
    while i > floor {
        i -= 1;
        let c = b[i];
        if c == b'"' && !escaped(b, i, floor) {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match c {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// DH.4: is the byte at `i` escaped, an odd run of `\` right before it
/// (counted no further back than `floor`)?
fn escaped(b: &[u8], i: usize, floor: usize) -> bool {
    let mut run = 0usize;
    let mut k = i;
    while k > floor && b.get(k - 1) == Some(&b'\\') {
        run += 1;
        k -= 1;
    }
    run % 2 == 1
}

/// CL.2: calls whose only job is to wrap a keyed value in a pointer, aws-sdk-go's
/// `aws.String("x")` for every `*string` field (`QueueUrl`, `TopicArn`). A
/// keyed value written through one is read INSIDE it, by [`keyed_literal`] and
/// [`keyed_expr`] alike. A FIXED list on purpose: a general `f("x")` unwrap
/// would read `QueueUrl: os.Getenv("QUEUE_URL")` as a queue named `QUEUE_URL`.
const POINTER_HELPERS: &[&str] = &["aws.String("];

/// CL.2: when the value at `j` opens a [`POINTER_HELPERS`] call, the byte just
/// inside it, whitespace skipped. Matched AT the value, so `myaws.String(` is
/// never the helper. The helper is ASCII, so the index stays on a char boundary.
fn step_into_helper(region: &str, j: usize) -> Option<usize> {
    let rest = region.get(j..)?;
    let helper = POINTER_HELPERS.iter().find(|h| rest.starts_with(**h))?;
    Some(skip_ws(region.as_bytes(), j + helper.len()))
}

// ---------------------------------------------------------------------------
// LA.4 (A11.7) — the identifier in a topic slot, for the engine's const fold
// ---------------------------------------------------------------------------

/// Bytes an identifier chain may continue with: `Topics.ORDERS`,
/// `Topics::ORDERS`, `$this->topic`, `config?.topic`, `cfg!.topic`.
fn is_expr_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'.' | b':' | b'>' | b'?' | b'!' | b'-')
}

/// Bytes that may end a keyed value: the next member, the close of the
/// object / array / call, a statement end, or the end of a paren-less line.
fn ends_value(c: Option<&u8>) -> bool {
    matches!(
        c,
        None | Some(b',' | b'}' | b']' | b')' | b';' | b'\n' | b'\r')
    )
}

/// The identifier chain at the start of `s`, with an optional trailing `()`,
/// and the byte length it spans. First byte `[A-Za-z_$@]`; a quote, a
/// whitespace, `+`, `[`, `{` or a `(` that is not exactly `()` ends it, and the
/// caller decides whether what follows is allowed. At most
/// [`MAX_TOPIC_LEN`] bytes. Every byte stepped over is ASCII, so the slice
/// lands on a char boundary.
fn ident_chain(s: &str) -> Option<(&str, usize)> {
    let b = s.as_bytes();
    let first = b.first()?;
    if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'$' | b'@')) {
        return None;
    }
    let mut i = 1usize;
    while matches!(b.get(i), Some(c) if is_expr_byte(*c)) {
        i += 1;
    }
    if b.get(i) == Some(&b'(') && b.get(i + 1) == Some(&b')') {
        i += 2;
    }
    if i > MAX_TOPIC_LEN {
        return None;
    }
    Some((s.get(..i)?, i))
}

/// The identifier expression that IS positional argument `n` — the slot
/// [`arg_literal`] reads. The whole trimmed argument must be one chain, so
/// `PREFIX + id`, `topicFor(a)`, `[TOPIC]` and every literal are `None`.
fn arg_expr(region: &str, n: usize) -> Option<String> {
    let arg = split_args(region).get(n)?.trim();
    let (expr, len) = ident_chain(arg)?;
    (len == arg.len()).then(|| expr.to_string())
}

/// The identifier expression after `<key> <sep>`, where [`keyed_literal`]
/// looks for its literal (same keys, same order, same quoted-key and builder
/// forms), or the key itself for an ES6 shorthand member (`{ topic, messages }`
/// — the key is preceded by `{` or `,` and followed by `,` or `}`). The value
/// must end the member: `topic: PREFIX + id` is `None`.
fn keyed_expr(region: &str, keys: &[&str]) -> Option<String> {
    let lower = region.to_ascii_lowercase();
    if lower.len() != region.len() {
        return None;
    }
    let b = region.as_bytes();
    for key in keys {
        let k = key.to_ascii_lowercase();
        if k.is_empty() {
            continue;
        }
        let mut from = 0usize;
        while let Some(rel) = lower.get(from..).and_then(|s| s.find(&k)) {
            let start = from + rel;
            let end = start + k.len();
            from = end;
            if !word_edge(b, start.checked_sub(1)) || !word_edge(b, Some(end)) {
                continue;
            }
            let mut j = end;
            let quoted_key = matches!(b.get(j), Some(b'\'' | b'"' | b'`'));
            if quoted_key {
                j += 1;
            }
            j = skip_ws(b, j);
            let builder = b.get(j) == Some(&b'(') && start > 0 && b.get(start - 1) == Some(&b'.');
            let Some(sep) = separator_len(b, j).or(builder.then_some(1)) else {
                if !quoted_key && shorthand_member(b, start, j) {
                    return region.get(start..end).map(str::to_string);
                }
                continue;
            };
            let value_at = skip_ws(b, j + sep);
            // CL.2: `QueueUrl: aws.String(queueURL)` hands `queueURL` to the
            // LA.4 fold; the helper's whole argument must be the chain.
            let helper = step_into_helper(region, value_at);
            let value_at = helper.unwrap_or(value_at);
            let Some(rest) = region.get(value_at..) else {
                continue;
            };
            let Some((expr, len)) = ident_chain(rest) else {
                continue;
            };
            let next = b.get(skip_ws(b, value_at + len));
            let ends = match helper {
                Some(_) => next == Some(&b')'),
                None => ends_value(next),
            };
            if ends {
                return Some(expr.to_string());
            }
        }
    }
    None
}

/// An ES6 shorthand member: the key at `start` opens a member (`{` or `,`
/// before it), `after_ws`, the first byte past it, closes one, and the
/// innermost bracket open at `start` is a `{` — so a bare positional argument
/// (`send(a, topic, b)`) is never read as a member.
fn shorthand_member(b: &[u8], start: usize, after_ws: usize) -> bool {
    let mut i = start;
    while i > 0 && b.get(i - 1).is_some_and(u8::is_ascii_whitespace) {
        i -= 1;
    }
    let opens = i > 0 && matches!(b.get(i - 1), Some(b'{' | b','));
    opens && matches!(b.get(after_ws), Some(b',' | b'}')) && innermost_open(b, start) == Some(b'{')
}

/// The innermost bracket still open at byte `at`, quotes and `\` escapes
/// respected (the same walk as [`split_args`]).
fn innermost_open(b: &[u8], at: usize) -> Option<u8> {
    let mut open: Vec<u8> = Vec::new();
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < at.min(b.len()) {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'{' | b'[' => open.push(c),
                b')' | b'}' | b']' => {
                    open.pop();
                }
                _ => {}
            }
        }
        i += 1;
    }
    open.last().copied()
}

// ---------------------------------------------------------------------------
// A2.5 — task-queue identity: the receiver / the definition / the enclosing type
// ---------------------------------------------------------------------------

/// Bytes that may appear in a symbol chain (`tasks.send_email`, `Sidekiq::Job`,
/// `$queue`). `.` and `:` are included so the chain is captured whole and split
/// afterwards — the last segment is the identity.
fn is_chain_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'.' | b':')
}

/// A chain segment, if it can be a task identity at all.
fn accept_symbol(seg: &str) -> Option<String> {
    if NOT_A_SYMBOL.contains(&seg.to_ascii_lowercase().as_str()) {
        return None;
    }
    let first = seg.as_bytes().first()?;
    if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'$')) {
        return None;
    }
    normalise_topic(seg)
}

/// Last acceptable segment of a `.`/`::` chain. Returns None rather than a
/// partial guess, so the caller falls through to the unresolved sentinel.
fn last_segment(chain: &str) -> Option<String> {
    let mut segs = chain.split(['.', ':']).filter(|s| !s.is_empty());
    accept_symbol(segs.next_back()?)
}

/// Like [`last_segment`] but steps back over rejected segments, so Oban's
/// `EmailWorker.new(%{...})` names the WORKER instead of failing on `new`.
fn last_symbol(chain: &str) -> Option<String> {
    chain
        .split(['.', ':'])
        .filter(|s| !s.is_empty())
        .rev()
        .find_map(accept_symbol)
}

/// Trailing symbol chain of `s`, reduced to its last segment.
fn ident_before(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut i = b.len();
    // Only ASCII bytes are stepped over, so `i` stays on a char boundary.
    while i > 0 && is_chain_byte(b[i - 1]) {
        i -= 1;
    }
    last_segment(s.get(i..)?)
}

/// Leading symbol chain of `s`, reduced to its last segment
/// (`MyApp.EmailWorker do` -> `EmailWorker`, `Foo:` -> `Foo`).
fn ident_at(s: &str) -> Option<String> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0usize;
    while matches!(b.get(i), Some(c) if is_chain_byte(*c)) {
        i += 1;
    }
    last_segment(t.get(..i)?)
}

/// CJ.1a: false when `needle` is a bare word (its first byte is an
/// identifier byte, Sidekiq's `perform_async` / `perform_in`) read by
/// [`TopicRule::Receiver`] and the occurrence at `offset` is part of a longer
/// identifier: an identifier byte right before it or right after the needle
/// (`sidekiq_perform_async_uses_class`, `MyJob.perform_inline(`).
/// `HardWorker.perform_async(` and `perform_async(1)` are sites; every other
/// row (`producer.send(`, `.delay(`) is unaffected.
///
/// DH.1: [`TopicRule::BareArg`] joins it. Karafka's `topic ` needle also
/// occurs inside `subtopic :x` and `my_topic "y"`, which are no route; the
/// after-edge test passes the quote or `:` a real `topic :orders` puts there.
///
/// DG.2: [`TopicRule::CtorArgLiteral`] joins it: the `ProducerRecord` type
/// needle also occurs inside `MyProducerRecord(` and `ProducerRecords`, which
/// construct no Kafka record; the after-edge test passes the `<`, `[` or `(`
/// a real constructor puts there.
fn bare_word_ok(source: &str, offset: usize, needle: &str, rule: TopicRule) -> bool {
    let bare = matches!(
        rule,
        TopicRule::Receiver | TopicRule::BareArg | TopicRule::CtorArgLiteral
    )
        && needle
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_');
    if !bare {
        return true;
    }
    let b = source.as_bytes();
    word_edge(b, offset.checked_sub(1)) && word_edge(b, Some(offset + needle.len()))
}

/// DH.1 [`TopicRule::BareArg`] — the first argument after `after`, spaces and
/// tabs skipped (never a newline: a paren-less call ends with its line). When
/// a `(` opens there it is argument #0 of that region, else the bytes right
/// there; either way the argument must START with a quoted literal or a
/// `:symbol` ([`read_literal`]): `topic :orders do`, `topic "orders" do`,
/// `topic (:orders)`. `topic = x`, `topic Topics::ORDERS`, `topic ::Orders`
/// and `topic (Foo.new("x"))` read nothing. Byte walks only: every index it
/// slices at follows an ASCII byte.
fn bare_arg(source: &str, after: usize) -> Option<Folded> {
    let b = source.as_bytes();
    let mut j = after;
    while matches!(b.get(j), Some(b' ' | b'\t')) {
        j += 1;
    }
    if b.get(j) == Some(&b'(') {
        let region = region_body(source.get(j + 1..)?)?;
        let arg = (*split_args(region).first()?).trim_start();
        return read_literal(arg, 0);
    }
    read_literal(source, j)
}

/// Bound on the lists [`command_arg`] steps into below the argument region:
/// `send_object(['PUBLISH', ..])` is one, Redix's pipeline
/// `[["PUBLISH", ..], ..]` two.
const MAX_COMMAND_DEPTH: usize = 3;

/// DH.3 [`TopicRule::CommandArg`] — the command vector in `region`. Walks the
/// region's arguments and, depth-first in order, the elements of every list
/// inside them ([`split_args`], so nesting and quoting are respected); the
/// first vector whose FIRST element is a literal equal to `cmd` (ASCII
/// case-insensitive) names the topic in its NEXT element. `Some(Some(t))`:
/// that element starts with a quoted literal, folded ([`fold_topic`]).
/// `Some(None)`: the command with no literal after it (a channel in a
/// variable), a site that reads no topic. `None`: no vector opens with the
/// command, so the call is no site.
fn command_arg(region: &str, cmd: &str) -> Option<Option<Folded>> {
    command_in(region, cmd, 0)
}

fn command_in(list: &str, cmd: &str, depth: usize) -> Option<Option<Folded>> {
    let elems: Vec<&str> = split_args(list).into_iter().map(str::trim).collect();
    let opens = elems
        .first()
        .and_then(|e| read_literal(e, 0))
        .is_some_and(|(verb, _)| verb.eq_ignore_ascii_case(cmd));
    if opens {
        return Some(elems.get(1).and_then(|e| read_literal(e, 0)));
    }
    if depth >= MAX_COMMAND_DEPTH {
        return None;
    }
    elems
        .iter()
        .find_map(|e| list_body(e).and_then(|inner| command_in(inner, cmd, depth + 1)))
}

/// DH.3: the body of a list literal element, `[..]`, or Dart's typed
/// `<Object>[..]` (one balanced `<..>` stepped over, [`skip_type_args`]).
fn list_body(elem: &str) -> Option<&str> {
    let t = if elem.starts_with('<') {
        skip_type_args(elem)?.trim_start()
    } else {
        elem
    };
    region_body(t.strip_prefix('[')?)
}

/// [`TopicRule::Receiver`] — the chain immediately before the needle.
fn receiver_before(source: &str, offset: usize) -> Option<String> {
    ident_before(source.get(..offset)?)
}

/// [`TopicRule::ArgReceiver`] — the callee of positional argument #0.
///
/// DH.4: when argument #0 is ABSENT (no region after the needle, or an empty
/// one), the job is read from the pipe stage before the needle instead
/// ([`piped_receiver`]): Oban's documented `%{..} |> MyApp.MailerWorker.new()
/// |> Oban.insert()`, and `Oban.insert!` / `Oban.insert_all` through the same
/// `Oban.insert` needle. A present argument #0 reads exactly as before, so
/// `Oban.insert(EmailWorker.new(%{}))` and `Oban.insert(changeset)` are
/// unchanged.
fn arg_receiver(source: &str, offset: usize, after: usize, needle: &str) -> Option<String> {
    let arg = arg_region(source, after, needle)
        .and_then(|region| split_args(region).first().copied())
        .filter(|arg| !arg.trim().is_empty());
    let Some(arg) = arg else {
        return piped_receiver(source, offset);
    };
    let call = arg.find('(')?;
    last_symbol(arg.get(..call)?)
}

/// DH.4: the job a pipe hands the needle at `offset`. Before the needle
/// (whitespace and line breaks skipped) must stand `|>`, and the stage before
/// that `|>` must END with `<Chain>.new(<args>)`: its last byte a `)` whose
/// matching `(` ([`unmatched_open_before`]) follows `.new`. The job is the
/// chain's last segment, `MyApp.MailerWorker.new(%{}) |> Oban.insert()` ->
/// `MailerWorker`, the argument list may span lines, and the stage may sit
/// on the line before. `x |> Repo.insert() |> Oban.insert()`, `jobs |>
/// Oban.insert_all()` and a needle no `|>` precedes read nothing. Byte walks
/// only: every index sliced at is an ASCII byte's.
fn piped_receiver(source: &str, offset: usize) -> Option<String> {
    let b = source.as_bytes();
    let mut i = offset.min(b.len());
    while i > 0 && b[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i < 2 || b.get(i - 2..i) != Some(b"|>".as_slice()) {
        return None;
    }
    i -= 2;
    while i > 0 && b[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i == 0 || b[i - 1] != b')' {
        return None;
    }
    let open = unmatched_open_before(b, i - 1)?;
    if b.get(open) != Some(&b'(') {
        return None;
    }
    ident_before(source.get(..open)?.strip_suffix(".new")?)
}

/// Byte just past `kw` in `line`, when `kw` starts at a word edge — so
/// `backoff_func = x` does not read as a `func ` definition.
fn find_kw(line: &str, kw: &str) -> Option<usize> {
    let b = line.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = line.get(from..).and_then(|s| s.find(kw)) {
        let start = from + rel;
        from = start + kw.len();
        if word_edge(b, start.checked_sub(1)) {
            return Some(from);
        }
    }
    None
}

/// The name a def-like line declares: the identifier just before the parameter
/// list (`public async Task HandleAsync(Foo f)` -> `HandleAsync`, not the return
/// type), else the one right after the keyword (`def perform`, `class Foo:`).
fn declared_name(line: &str, kw_end: usize) -> Option<String> {
    let tail = line.get(kw_end..)?;
    match tail.find('(') {
        Some(p) => ident_before(tail.get(..p)?).or_else(|| ident_at(tail)),
        None => ident_at(tail),
    }
}

/// [`TopicRule::DeclaredSymbol`] — the definition under a decorator. Non-def
/// lines are SKIPPED, not failed, so a stack of decorators
/// (`@shared_task` / `@retry(...)` / `def send_email`) still finds the def.
fn declared_symbol(source: &str, after: usize) -> Option<String> {
    declared_line(source, after).map(|(_, name)| name)
}

/// The definition line [`declared_symbol`] reads, with the name it declares.
fn declared_line(source: &str, after: usize) -> Option<(&str, String)> {
    let rest = source.get(after..)?;
    let body = rest.get(rest.find('\n')? + 1..)?;
    for line in body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(LOOKAHEAD_LINES)
    {
        let Some(kw_end) = DEF_KEYWORDS.iter().find_map(|k| find_kw(line, k)) else {
            continue;
        };
        if let Some(name) = declared_name(line, kw_end) {
            return Some((line, name));
        }
    }
    None
}

/// CL.4: does the definition [`TopicRule::DeclaredSymbol`] reads under the
/// needle ending at `after` declare a TYPE rather than a method? True when a
/// [`TYPE_KEYWORDS`] word sits before the line's `(`: a class-level Hangfire
/// `[AutomaticRetry]` over `public class EmailJobs`, `[Queue] public record
/// Job(string Id)`. For a framework whose job is a METHOD the extractor reads
/// such a hit as naming no job.
pub(crate) fn declares_type(source: &str, after: usize) -> bool {
    declared_line(source, after).is_some_and(|(line, _)| {
        let head = line.split('(').next().unwrap_or(line);
        TYPE_KEYWORDS.iter().any(|k| find_kw(head, k).is_some())
    })
}

/// [`TopicRule::EnclosingSymbol`] — the nearest scope above the needle. The
/// needle's own line is included, so `class Foo; include Sidekiq::Worker; end`
/// resolves too.
fn enclosing_symbol(source: &str, offset: usize) -> Option<String> {
    let head = source.get(..offset)?;
    for line in head.lines().rev().take(ENCLOSING_LOOKBACK_LINES) {
        let Some(kw_end) = SCOPE_KEYWORDS.iter().find_map(|k| find_kw(line, k)) else {
            continue;
        };
        if let Some(name) = line.get(kw_end..).and_then(ident_at) {
            return Some(name);
        }
    }
    None
}

/// CL.4 [`TopicRule::LambdaCallee`] — the method a job lambda calls. In the
/// needle's argument region ([`opened_region`], so CL.3's `<T>` skip applies)
/// the body is what follows the first depth-0 `->` / `=>`; a Kotlin trailing
/// lambda is a body as it stands, written alone (`enqueue { svc.run() }`) or
/// after the arguments on the same line (`schedule(at) { svc.run() }`).
/// [`lambda_body_callee`] reads the body's callee. A method reference
/// (`enqueue(svc::run)`) or a plain argument (`queue.Enqueue(item)`) has no
/// lambda and reads nothing.
fn lambda_callee(source: &str, after: usize, needle: &str) -> Option<String> {
    let (open, region) = opened_region(source, after, needle)?;
    if let Some(at) = depth0_arrow(region) {
        return lambda_body_callee(region.get(at + 2..)?);
    }
    if open == b'{' {
        return lambda_body_callee(region);
    }
    // The byte after the region is its `)` when the call closed; `region` is
    // a subslice of `source`, so its offset is the pointer difference.
    let end = (region.as_ptr() as usize)
        .checked_sub(source.as_ptr() as usize)?
        .checked_add(region.len())?;
    let trailing = source
        .get(end..)?
        .strip_prefix(')')?
        .trim_start_matches([' ', '\t'])
        .strip_prefix('{')?;
    let body = region_body(trailing)?;
    match depth0_arrow(body) {
        Some(at) => lambda_body_callee(body.get(at + 2..)?),
        None => lambda_body_callee(body),
    }
}

/// CL.4: byte index of the first `->` / `=>` at bracket depth 0 of `region`,
/// quoted text stepped over ([`region_body`]'s rules). Both arrows are ASCII.
fn depth0_arrow(region: &str) -> Option<usize> {
    let b = region.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'{' | b'[' => depth += 1,
                b')' | b'}' | b']' => depth -= 1,
                b'-' | b'=' if depth == 0 && b.get(i + 1) == Some(&b'>') => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// CL.4: the callee a lambda body opens with — past an optional block `{`,
/// `return` and `await`, an identifier chain followed by `(`, reduced to its
/// last segment ([`last_segment`]). `() -> 42`, `() -> new Foo()` and
/// `x => x.Run<int>()` read nothing.
fn lambda_body_callee(body: &str) -> Option<String> {
    let mut t = body.trim_start();
    if let Some(rest) = t.strip_prefix('{') {
        t = rest.trim_start();
    }
    for kw in ["return", "await"] {
        if let Some(rest) = t
            .strip_prefix(kw)
            .filter(|r| r.starts_with(|c: char| c.is_ascii_whitespace()))
        {
            t = rest.trim_start();
        }
    }
    let len = t
        .bytes()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'.'))
        .count();
    let chain = t.get(..len)?;
    if !t.get(len..)?.trim_start().starts_with('(') {
        return None;
    }
    last_segment(chain)
}

// ---------------------------------------------------------------------------
// DG.1 — apalis typed jobs: the job is a Rust TYPE, read off declarations
// ---------------------------------------------------------------------------

/// DG.1: path segments that never name a job type — `Self` and the std
/// wrappers a value or type may be spelled through.
const NOT_A_JOB_TYPE: &[&str] = &[
    "Self", "Some", "None", "Ok", "Err", "Box", "Arc", "Rc", "Vec", "String", "Option", "Result",
];

/// DG.1 [`TopicRule::StorageJobType`] — the job an apalis storage push names.
///
/// The receiver R (the identifier right before the needle, a line-broken
/// chain allowed, [`word_before`]) must be DECLARED in this file
/// ([`rust_binding_type`]): a parameter / field `R: <ty>` or a `let [mut]
/// R[: <ty>] = <init>` whose type or initializer path names a segment ending
/// in `Storage` (`&mut apalis_redis::RedisStorage<Job>`,
/// `MemoryStorage::new()`, `PostgresStorage::new(pool)`, `impl Storage<Job =
/// Email>`). The job is that storage's type argument ([`storage_generic`]),
/// else the type the pushed value is built from ([`pushed_type`]).
///
/// `None` — no site — for a `Vec`'s push, a call-result receiver, a pushed
/// variable into an untyped storage, and a storage declared in ANOTHER file:
/// one passed in through a struct field declared elsewhere
/// (`self.storage.push(job)`) is not read (a field declared in this file is
/// an `R: <ty>` declaration and is).
fn storage_job_type(source: &str, offset: usize, after: usize, needle: &str) -> Option<String> {
    let (recv, _) = word_before(source, offset)?;
    if recv == "self" || recv.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let generic = storage_generic(rust_binding_type(source, recv, offset)?)?;
    generic
        .and_then(rust_type_name)
        .or_else(|| pushed_type(source, after, needle))
}

/// DG.1 [`TopicRule::WorkerJobType`] — the job an apalis worker runs.
///
/// The needle's statement ([`statement_start`]) must hold `WorkerBuilder`
/// before the needle, else `None` (`.build(` is every builder's method). The
/// job is the first parameter type of the handler fn positional argument #0
/// names, declared in this file (`async fn send_email(job: Email, ..)`,
/// [`rust_fn_first_param_type`]); else the type argument of the statement's
/// `.backend(S)` (apalis 0.6+) / `.with_storage(S)` (0.4 / 0.5) storage,
/// written inline (`RedisStorage::<Email>::new(c)`) or declared in this file
/// (`storage: RedisStorage<Email>`); else `None`.
fn worker_job_type(source: &str, offset: usize, after: usize, needle: &str) -> Option<String> {
    let stmt = source.get(statement_start(source, offset)..offset)?;
    if !stmt.contains("WorkerBuilder") {
        return None;
    }
    let handler = arg_region(source, after, needle)
        .and_then(|region| split_args(region).first().map(|a| a.trim()))
        .filter(|h| {
            h.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && h.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':'))
        });
    if let Some(job) = handler
        .and_then(|h| h.rsplit("::").next())
        .and_then(|f| rust_fn_first_param_type(source, f))
    {
        return Some(job);
    }
    let (at, len) = [".backend(", ".with_storage("]
        .iter()
        .filter_map(|k| stmt.rfind(k).map(|at| (at, k.len())))
        .max()?;
    let arg = split_args(region_body(stmt.get(at + len..)?)?)
        .first()?
        .trim();
    if let Some(job) = storage_generic(arg).flatten().and_then(rust_type_name) {
        return Some(job);
    }
    let (ident, _) = *rust_path(arg).first()?;
    storage_generic(rust_binding_type(source, ident, offset)?)?.and_then(rust_type_name)
}

/// DG.1: the declaration text of the binding `ident`: the `<ty>` of a
/// parameter / field / typed `let` (`ident: <ty>`), else the initializer of
/// an untyped `let [mut] ident = <init>` ([`binding_decl`]). The nearest
/// declaration before `before` wins (the innermost one a straight-line read
/// sees), else the first after it (a struct field declared below its
/// `impl`). An occurrence on a `//` comment line declares nothing, so `//
/// storage: the redis handle` shadows no real declaration; the rest of the
/// file is read raw, which is safe because it only qualifies an occurrence
/// the CJ.1a guard already admitted as code.
fn rust_binding_type<'a>(source: &'a str, ident: &str, before: usize) -> Option<&'a str> {
    let b = source.as_bytes();
    let mut nearest = None;
    let mut later = None;
    for (at, _) in source.match_indices(ident) {
        let end = at + ident.len();
        if !word_edge(b, at.checked_sub(1)) || !word_edge(b, Some(end)) {
            continue;
        }
        // A member or path segment (`x.storage`, `m::storage`) declares nothing.
        if at > 0 && matches!(b[at - 1], b'.' | b':') {
            continue;
        }
        let line = source
            .get(..at)
            .and_then(|head| head.rsplit('\n').next())
            .unwrap_or("");
        if line.contains("//") {
            continue;
        }
        let Some(decl) = binding_decl(source, at, end) else {
            continue;
        };
        if at < before {
            nearest = Some(decl);
        } else if later.is_none() {
            later = Some(decl);
        }
    }
    nearest.or(later)
}

/// DG.1: what the occurrence of a binding at bytes `at..end` declares — the
/// type after `ident:` (never `ident::`), or the initializer after `let [mut]
/// ident =` (never `==` / `=>`) — or `None` when it declares nothing.
fn binding_decl(source: &str, at: usize, end: usize) -> Option<&str> {
    let b = source.as_bytes();
    let j = skip_ws(b, end);
    if b.get(j) == Some(&b':') && b.get(j + 1) != Some(&b':') {
        return Some(decl_text(source, j + 1, false)).filter(|t| !t.is_empty());
    }
    let let_bound = match word_before(source, at) {
        Some(("let", _)) => true,
        Some(("mut", m)) => matches!(word_before(source, m), Some(("let", _))),
        _ => false,
    };
    if !let_bound || b.get(j) != Some(&b'=') || matches!(b.get(j + 1), Some(b'=' | b'>')) {
        return None;
    }
    Some(decl_text(source, j + 1, true)).filter(|t| !t.is_empty())
}

/// DG.1: the declared type (`init` false) or initializer (`init` true) that
/// starts at `start`, trimmed: up to the first depth-0 `,` `)` `;` `=` `{`
/// `|` (a type) or `;` (an initializer), or the close of a bracket it did not
/// open. A type counts `<..>` as brackets (the `>` of `->` is none); an
/// initializer does not, a `<` there may be a comparison. `"` strings are
/// stepped over; at most [`MAX_REGION`] bytes are read, and every byte it
/// stops at is ASCII, so the slice lands on a char boundary.
fn decl_text(source: &str, start: usize, init: bool) -> &str {
    let Some(rest) = source.get(start..) else {
        return "";
    };
    let b = rest.as_bytes();
    let limit = b.len().min(MAX_REGION);
    let mut depth = 0i32;
    let mut quote = false;
    let mut i = 0usize;
    while i < limit {
        let c = b[i];
        if quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            quote = c != b'"';
            i += 1;
            continue;
        }
        let stop = match c {
            b'"' => {
                quote = true;
                false
            }
            b'(' | b'[' => {
                depth += 1;
                false
            }
            b'{' if init || depth > 0 => {
                depth += 1;
                false
            }
            b'{' => true,
            b'<' if !init => {
                depth += 1;
                false
            }
            // The `>` of `->` closes nothing.
            b'>' if !(init || (i > 0 && b[i - 1] == b'-')) => {
                depth -= 1;
                depth < 0
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                depth < 0
            }
            b',' | b'=' | b'|' if !init => depth == 0,
            b';' => depth == 0,
            _ => false,
        };
        if stop {
            return rest.get(..i).unwrap_or("").trim();
        }
        i += 1;
    }
    clip(rest, limit).trim()
}

/// DG.1: does the type or initializer path `decl` name a storage, a segment
/// ending in `Storage`? `Some(type argument)` when it does
/// (`RedisStorage<Job>`, the turbofish `RedisStorage::<Job>::new(c)`, `impl
/// Storage<Job = Email>` -> `Email`; [`first_type_arg`]), `Some(None)` for
/// one written without (`MemoryStorage::new()`), `None` when no segment is a
/// storage.
fn storage_generic(decl: &str) -> Option<Option<&str>> {
    let (_, args) = rust_path(decl)
        .into_iter()
        .find(|(seg, _)| seg.ends_with("Storage"))?;
    Some(args.and_then(first_type_arg))
}

/// DG.1: the first argument of a `<..>` list's inside (`Job, Codec<X>` ->
/// `Job`), an associated binding read as its type (`Job = Email` -> `Email`).
fn first_type_arg(args: &str) -> Option<&str> {
    let b = args.as_bytes();
    let mut depth = 0i32;
    let mut end = b.len();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' if !(i > 0 && b[i - 1] == b'-') => depth -= 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let arg = args.get(..end)?.trim();
    let arg = match arg.split_once('=') {
        Some((name, ty))
            if !name.trim().is_empty()
                && name
                    .trim()
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_') =>
        {
            ty.trim()
        }
        _ => arg,
    };
    (!arg.is_empty()).then_some(arg)
}

/// DG.1: the leading Rust path of `s`, `&` / `mut` / `dyn` / `impl` and
/// lifetimes stepped over: each segment with the inside of the `<..>` that
/// follows it (a type's arguments, or a turbofish `::<..>`), in order.
/// `&mut apalis_redis::RedisStorage<crate::Job>` -> `[(apalis_redis, None),
/// (RedisStorage, Some("crate::Job"))]`. Stops at the first byte no path
/// holds (`(`, `{`, `.`, a space); an unclosed `<` ends it after its segment.
fn rust_path(s: &str) -> Vec<(&str, Option<&str>)> {
    let mut rest = s.trim_start();
    loop {
        let before = rest.len();
        rest = rest.trim_start_matches('&').trim_start();
        if let Some(r) = rest.strip_prefix('\'') {
            let n = r
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                .count();
            rest = r.get(n..).unwrap_or("").trim_start();
        }
        for kw in ["mut ", "dyn ", "impl "] {
            if let Some(r) = rest.strip_prefix(kw) {
                rest = r.trim_start();
            }
        }
        if rest.len() == before {
            break;
        }
    }
    let mut out = Vec::new();
    loop {
        let n = rest
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
            .count();
        let Some(seg) = rest.get(..n).filter(|seg| !seg.is_empty()) else {
            break;
        };
        rest = rest.get(n..).unwrap_or("");
        let open = rest
            .strip_prefix("::")
            .filter(|r| r.starts_with('<'))
            .unwrap_or(rest);
        let mut args = None;
        if open.starts_with('<') {
            let Some((inside, tail)) = angle_body(open) else {
                out.push((seg, None));
                break;
            };
            args = Some(inside);
            rest = tail;
        }
        out.push((seg, args));
        match rest.strip_prefix("::") {
            Some(r) => rest = r,
            None => break,
        }
    }
    out
}

/// DG.1: `s` (which opens with `<`) split at its balanced close: the inside
/// and the text after the `>`. The `>` of `->` closes nothing; `None` when a
/// `;` / `{` / `}` comes first or [`MAX_REGION`] bytes pass (that `<` was a
/// comparison). Every byte it splits at is ASCII.
fn angle_body(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    for (i, &c) in b.iter().enumerate().take(MAX_REGION) {
        match c {
            b'<' => depth += 1,
            b'>' if !(i > 0 && b[i - 1] == b'-') => {
                depth -= 1;
                if depth == 0 {
                    return Some((s.get(1..i)?, s.get(i + 1..)?));
                }
            }
            b';' | b'{' | b'}' => return None,
            _ => {}
        }
    }
    None
}

/// DG.1: the job type a Rust type path names: its FIRST segment written with
/// an upper-case initial, so a module prefix is stepped over
/// (`crate::jobs::Email` -> `Email`) and an enum variant names its enum
/// (`Job::Email { .. }` -> `Job`). `None` for a one-letter name (a generic
/// parameter, `RedisStorage<T>`), a [`NOT_A_JOB_TYPE`] segment, and a path
/// with no upper-case segment.
fn rust_type_name(t: &str) -> Option<String> {
    let (seg, _) = rust_path(t)
        .into_iter()
        .find(|(seg, _)| seg.starts_with(|c: char| c.is_ascii_uppercase()))?;
    if seg.len() < 2 || NOT_A_JOB_TYPE.contains(&seg) {
        return None;
    }
    normalise_topic(seg)
}

/// DG.1: the type the value pushed as positional argument #0 is built from —
/// a struct / tuple-struct / unit literal or an associated constructor
/// (`Email { .. }`, `Email(..)`, `jobs::Email::new(..)`, `Email::default()`),
/// read by [`rust_type_name`]. A variable (`storage.push(job)`) and a
/// SCREAMING_CASE constant (`storage.push(DEFAULT_JOB)`) name no type.
fn pushed_type(source: &str, after: usize, needle: &str) -> Option<String> {
    let region = arg_region(source, after, needle)?;
    let arg = split_args(region).first()?.trim();
    rust_type_name(arg).filter(|t| t.bytes().any(|c| c.is_ascii_lowercase()))
}

/// DG.1: the type of the first non-`self` parameter of `fn <fn_name>`
/// declared in this file (`async fn send_email(job: Email, ctx: Data<..>)` ->
/// `Email`, a destructuring pattern's type too), read by [`rust_type_name`].
/// The first declaration whose parameter list reads wins.
fn rust_fn_first_param_type(source: &str, fn_name: &str) -> Option<String> {
    let b = source.as_bytes();
    for (at, _) in source.match_indices(fn_name) {
        let end = at + fn_name.len();
        if !word_edge(b, at.checked_sub(1)) || !word_edge(b, Some(end)) {
            continue;
        }
        if !matches!(word_before(source, at), Some(("fn", _))) {
            continue;
        }
        let mut rest = source.get(end..).unwrap_or("").trim_start();
        if rest.starts_with('<') {
            let Some((_, tail)) = angle_body(rest) else {
                continue;
            };
            rest = tail.trim_start();
        }
        let Some(params) = rest.strip_prefix('(').and_then(region_body) else {
            continue;
        };
        let param = split_args(params)
            .into_iter()
            .map(str::trim)
            .find(|p| !matches!(rust_path(p).first(), Some(("self", _))))?;
        let colon = depth0_colon(param)?;
        return rust_type_name(param.get(colon + 1..)?);
    }
    None
}

/// DG.1: byte index of the first `:` of `param` outside brackets that is not
/// half of a `::` — the one between a parameter's pattern and its type.
fn depth0_colon(param: &str) -> Option<usize> {
    let b = param.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            b':' if b.get(i + 1) == Some(&b':') => {
                i += 2;
                continue;
            }
            b':' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// DG.1: the identifier word ending just before `at`, whitespace (a line
/// break included) skipped, with its start offset: `let` of `let mut x`,
/// `storage` of `storage\n    .push(`. `None` when a non-identifier byte
/// comes first (`get().push(`).
fn word_before(source: &str, at: usize) -> Option<(&str, usize)> {
    let head = source.get(..at)?.trim_end();
    let b = head.as_bytes();
    let mut i = b.len();
    // Only ASCII bytes are stepped over, so `i` stays on a char boundary.
    while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
        i -= 1;
    }
    head.get(i..).filter(|w| !w.is_empty()).map(|w| (w, i))
}

/// DG.1: the byte where the statement holding `offset` starts — just past
/// the previous `;`, `{` or `}` outside every bracket the walk back from
/// `offset` sits in, so `Monitor::new().register(WorkerBuilder::new("x")
/// .backend(s).build_fn(h))` reads from `Monitor`. `"` strings are stepped
/// over; at most [`MAX_REGION`] bytes are walked, and the start is moved
/// forward to a char boundary.
fn statement_start(source: &str, offset: usize) -> usize {
    let b = source.as_bytes();
    let end = offset.min(b.len());
    let floor = end.saturating_sub(MAX_REGION);
    let mut depth = 0i32;
    let mut quote = false;
    let mut i = end;
    while i > floor {
        let c = b[i - 1];
        if quote {
            quote = !(c == b'"' && !(i >= 2 && b[i - 2] == b'\\'));
        } else {
            match c {
                b'"' => quote = true,
                b')' | b']' => depth += 1,
                b'(' | b'[' => depth -= 1,
                b'}' | b'{' | b';' if depth <= 0 => return i,
                b'}' => depth += 1,
                b'{' => depth -= 1,
                _ => {}
            }
        }
        i -= 1;
    }
    while i < end && !source.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// True when the byte at `at` is absent or is not part of an identifier — so
/// `topic` matches in `{Topic:` but not inside `TopicPartition`.
fn word_edge(b: &[u8], at: Option<usize>) -> bool {
    match at.and_then(|i| b.get(i)) {
        None => true,
        Some(c) => !(c.is_ascii_alphanumeric() || *c == b'_'),
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while matches!(b.get(i), Some(c) if c.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// Length of a key/value separator at `i`: `=>`, `:=`, `:` or `=`.
fn separator_len(b: &[u8], i: usize) -> Option<usize> {
    match (b.get(i), b.get(i + 1)) {
        (Some(b'='), Some(b'>')) => Some(2),
        (Some(b':'), Some(b'=')) => Some(2),
        (Some(b':'), _) => Some(1),
        (Some(b'='), _) => Some(1),
        _ => None,
    }
}

/// Read a quoted literal (`'x'`, `"x"`, `` `x` ``) or a bare atom (`:x`) at `j`.
///
/// A2.6: a one-byte string prefix is stepped over first — Python `f"..."`,
/// `r"..."`, `b"..."`, `u"..."` and C# `$"..."` / `@"..."` — so a keyed value
/// written as an interpolated queue URL still reaches [`fold_topic`], which
/// keeps the name when only the host/account were interpolated and rejects a
/// placeholder name.
fn read_literal(s: &str, mut j: usize) -> Option<Folded> {
    let b = s.as_bytes();
    if matches!(
        b.get(j),
        Some(b'f' | b'r' | b'b' | b'u' | b'F' | b'R' | b'B' | b'U' | b'$' | b'@')
    ) && matches!(b.get(j + 1), Some(b'\'' | b'"'))
    {
        j += 1;
    }
    match b.get(j) {
        Some(&q @ (b'\'' | b'"' | b'`')) => {
            j += 1;
            let start = j;
            while let Some(&c) = b.get(j) {
                if c == b'\\' {
                    j += 2;
                    continue;
                }
                if c == q {
                    return fold_topic(s.get(start..j)?);
                }
                j += 1;
            }
            None
        }
        // Ruby/Elixir symbol: `queue: :orders`
        Some(b':') => {
            j += 1;
            let start = j;
            while matches!(b.get(j), Some(c) if c.is_ascii_alphanumeric()
                || matches!(*c, b'_' | b'-' | b'.'))
            {
                j += 1;
            }
            if j == start {
                None
            } else {
                fold_topic(s.get(start..j)?)
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// DT.2c — the NATS subject grammar
// ---------------------------------------------------------------------------

/// DT.2c: the token a dynamic piece of a NATS PUBLISH subject folds to. A
/// publisher cannot publish to a wildcard subject, so a `*` token in a
/// QUEUE_PRODUCER topic is unambiguously this fold's marker, and the queue
/// resolver pairs such a producer through the NATS wildcard walk only.
pub const SUBJECT_PLACEHOLDER: &str = "*";

/// DT.2c: how a NATS publish subject was folded to its `*`-token form, the
/// `[queues] subject-fold` marker's counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectFold {
    /// Top-level `+` pieces: `"shop.shipments." + id`.
    Concat,
    /// One interpolated literal: `` `orders.${id}.placed` ``,
    /// `f"orders.{order_id}.placed"`, `"orders.#{id}"`, `"orders.$id"`.
    Template,
    /// `fmt.Sprintf("shop.orders.%s.placed", id)`.
    Sprintf,
    /// A same-file one-return subject function: `shipment(id)`, whose body is
    /// `return "shop.shipments." + id`.
    Func,
}

/// DT.2c: what a NATS subject slot reads as.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubjectRead {
    /// One quoted literal with no interpolation, through [`fold_topic`].
    Literal(Folded),
    /// A subject whose whole dynamic tokens folded to `*`.
    Folded { topic: String, how: SubjectFold },
    /// LA.4: an identifier chain (`format: false`), or the identifier-chain
    /// format of a `fmt.Sprintf` (`format: true`), for the post-cache fold.
    Expr { chain: String, format: bool },
    /// A shape no rule reads: the unresolved tag.
    Dynamic,
}

/// DT.2c: one piece of a subject: literal text, or a value only known at run
/// time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Piece<'a> {
    Lit(&'a str),
    Dyn,
}

/// DT.2c: how a quoted literal interpolates, read off its prefix and quote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interp {
    /// Python `f"..{x}.."`, C# `$"..{x}.."`: `{..}` spans, `{{` / `}}` literal.
    Brace,
    /// JS / TS backtick: `${..}` spans.
    Template,
    /// Kotlin / Scala / Dart / Groovy `${..}` and `$name`, Ruby `#{..}` (in a
    /// `"` literal only). A `$NAME` whose name holds no lower-case letter is
    /// text: NATS system subjects (`$JS.API..`, `$KV.<bucket>..`) begin so.
    Dollar,
    /// Python `r` / `b` / `u`, C# `@`: no interpolation.
    Raw,
}

/// DT.2c: the NATS PUBLISH subject `arg` (one trimmed argument, the slot
/// [`nats_subject_slot`] picks) reads as. `source` is the whole file, read
/// for a same-file one-return subject function ([`go_single_return`]).
/// Shapes, in order:
///
/// * a named argument (`subject="x"`, `subject: "x"`) reads its value;
/// * top-level `+` pieces ([`split_plus`], quotes and brackets respected):
///   each quoted literal is literal text (its interpolations dynamic), any
///   other piece dynamic, then [`subject_from_pieces`] (`Concat`);
/// * one quoted literal: no interpolation is the [`SubjectRead::Literal`];
///   with one, its spans are dynamic pieces (`Template`);
/// * `fmt.Sprintf(<fmt>, ..)`: a literal format through
///   [`subject_from_format`] (`Sprintf`); an identifier-chain format is the
///   LA.4 expression with `format: true`;
/// * a call `name(..)` whose callee is a bare identifier naming a same-file
///   top-level `func name(..) string { return <expr> }`: `<expr>` read by the
///   same rules at depth 1, where every non-literal (its parameters,
///   `strings.TrimSpace(x)`, a constant) is a dynamic piece and no further
///   function is followed (`Func`);
/// * an identifier chain (`subject`, `cfg.Subject`, `subject()`): the LA.4
///   expression, as before;
/// * anything else: [`SubjectRead::Dynamic`].
pub(crate) fn nats_subject(arg: &str, source: &str) -> SubjectRead {
    subject_read(named_value(arg.trim()), source, 0)
}

/// DT.2c: the NATS SUBSCRIBE filter `arg` reads as. A literal with no
/// interpolation, or an identifier chain, reads as it always has; any
/// concatenation, interpolation, format or call is [`SubjectRead::Dynamic`]
/// and is never folded: a filter's `*` is a real wildcard, a runtime token is
/// not one.
pub(crate) fn nats_filter(arg: &str) -> SubjectRead {
    let arg = named_value(arg.trim());
    if let Some(lit) = quoted_literal(arg) {
        if lit.interpolates() {
            return SubjectRead::Dynamic;
        }
        return literal_read(arg, lit);
    }
    match ident_chain(arg) {
        Some((chain, len)) if len == arg.len() => SubjectRead::Expr {
            chain: chain.to_string(),
            format: false,
        },
        _ => SubjectRead::Dynamic,
    }
}

/// DT.2c: [`nats_subject`] at `depth` (0 at the call site, 1 inside a
/// one-return subject function).
fn subject_read(arg: &str, source: &str, depth: usize) -> SubjectRead {
    let arg = arg.trim();
    if arg.is_empty() {
        return SubjectRead::Dynamic;
    }
    let parts = split_plus(arg);
    if parts.len() > 1 {
        let mut pieces: Vec<Piece<'_>> = Vec::new();
        for part in &parts {
            let part = part.trim();
            match quoted_literal(part) {
                Some(lit) => match lit.pieces() {
                    Some(p) => pieces.extend(p),
                    None => return SubjectRead::Dynamic,
                },
                None => pieces.push(Piece::Dyn),
            }
        }
        return folded(subject_from_pieces(&pieces), SubjectFold::Concat);
    }
    if let Some(lit) = quoted_literal(arg) {
        if !lit.interpolates() {
            return literal_read(arg, lit);
        }
        let pieces = lit.pieces().unwrap_or_default();
        return folded(subject_from_pieces(&pieces), SubjectFold::Template);
    }
    if let Some(inner) = call_args(arg, "fmt.Sprintf") {
        let fmt = split_args(inner).first().map_or("", |a| a.trim());
        if let Some(lit) = quoted_literal(fmt) {
            return folded(subject_from_format(lit.body), SubjectFold::Sprintf);
        }
        return match ident_chain(fmt) {
            Some((chain, len)) if depth == 0 && len == fmt.len() => SubjectRead::Expr {
                chain: chain.to_string(),
                format: true,
            },
            _ => SubjectRead::Dynamic,
        };
    }
    if depth > 0 {
        return SubjectRead::Dynamic;
    }
    if let Some(body) = bare_callee(arg).and_then(|name| go_single_return(source, name)) {
        return match subject_read(&body, source, depth + 1) {
            SubjectRead::Literal((topic, _)) | SubjectRead::Folded { topic, .. } => {
                SubjectRead::Folded {
                    topic,
                    how: SubjectFold::Func,
                }
            }
            _ => SubjectRead::Dynamic,
        };
    }
    match ident_chain(arg) {
        Some((chain, len)) if len == arg.len() => SubjectRead::Expr {
            chain: chain.to_string(),
            format: false,
        },
        _ => SubjectRead::Dynamic,
    }
}

fn folded(topic: Option<String>, how: SubjectFold) -> SubjectRead {
    match topic {
        Some(topic) => SubjectRead::Folded { topic, how },
        None => SubjectRead::Dynamic,
    }
}

/// DT.2c: a literal with no interpolation read exactly as
/// [`TopicRule::ArgLiteral`] reads it ([`read_literal`] at its quote); a
/// literal [`fold_topic`] refuses (too long, a newline) is unreadable.
fn literal_read(arg: &str, lit: QuotedLiteral<'_>) -> SubjectRead {
    match read_literal(arg, lit.quote_at) {
        Some(f) => SubjectRead::Literal(f),
        None => SubjectRead::Dynamic,
    }
}

/// DT.2c: the value of a named argument (`subject="orders"` in Python /
/// Kotlin / Scala, `subject: "orders"` in C# / Dart / Ruby), else `arg`. The
/// name is a bare identifier followed by `=` (never `==` / `=>`) or `:`
/// (never `::`), so a ternary, a comparison and a path are left alone.
fn named_value(arg: &str) -> &str {
    let b = arg.as_bytes();
    if !matches!(b.first(), Some(c) if c.is_ascii_alphabetic() || *c == b'_') {
        return arg;
    }
    let mut i = 1usize;
    while matches!(b.get(i), Some(c) if c.is_ascii_alphanumeric() || *c == b'_') {
        i += 1;
    }
    let j = skip_ws(b, i);
    let sep = match (b.get(j), b.get(j + 1)) {
        (Some(b'='), Some(b'=' | b'>')) | (Some(b':'), Some(b':')) => return arg,
        (Some(b'=' | b':'), _) => j + 1,
        _ => return arg,
    };
    // Every byte stepped over is ASCII, so `sep` is a char boundary.
    arg.get(sep..).map_or(arg, str::trim)
}

/// DT.2c: `arg` split on its top-level `+` operators: quotes (with `\`
/// escapes) and brackets respected, so `${a + b}` and `f(a + b)` stay one
/// piece. A `+` right after another `+` or at either end makes no pieces of
/// its own (empty pieces are dropped by the caller's literal test).
fn split_plus(arg: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = arg.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'{' | b'[' => depth += 1,
                b')' | b'}' | b']' => depth -= 1,
                b'+' if depth == 0 => {
                    if let Some(piece) = arg.get(start..i) {
                        out.push(piece);
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if let Some(piece) = arg.get(start..) {
        out.push(piece);
    }
    out
}

/// DT.2c: one quoted literal that is a WHOLE piece: an optional one- or
/// two-byte prefix (`f`, `r`, `b`, `u`, `s`, `$`, `@`, either case), the
/// quote, a body with no unescaped closing quote, and the closing quote as
/// the piece's last byte.
#[derive(Clone, Copy, Debug)]
struct QuotedLiteral<'a> {
    /// Byte index of the opening quote in the piece.
    quote_at: usize,
    quote: u8,
    body: &'a str,
    interp: Interp,
}

fn quoted_literal(piece: &str) -> Option<QuotedLiteral<'_>> {
    let b = piece.as_bytes();
    let mut q = 0usize;
    while q < 2 && matches!(b.get(q), Some(c) if is_string_prefix(*c)) {
        q += 1;
    }
    let quote = *b.get(q).filter(|c| matches!(c, b'\'' | b'"' | b'`'))?;
    if b.len() < q + 2 || b.last() != Some(&quote) {
        return None;
    }
    let end = b.len() - 1;
    let mut i = q + 1;
    while i < end {
        match b[i] {
            b'\\' => i += 2,
            c if c == quote => return None,
            _ => i += 1,
        }
    }
    if i != end {
        return None;
    }
    let prefix = piece.get(..q)?;
    let interp = if prefix.contains(['f', 'F', '$']) {
        Interp::Brace
    } else if !prefix.is_empty() && !prefix.contains(['s', 'S']) {
        Interp::Raw
    } else if quote == b'`' {
        Interp::Template
    } else {
        Interp::Dollar
    };
    Some(QuotedLiteral {
        quote_at: q,
        quote,
        body: piece.get(q + 1..end)?,
        interp,
    })
}

/// A string-literal prefix byte: Python `f r b u`, Scala `s`, C# `$ @`.
fn is_string_prefix(c: u8) -> bool {
    matches!(
        c,
        b'f' | b'F' | b'r' | b'R' | b'b' | b'B' | b'u' | b'U' | b's' | b'S' | b'$' | b'@'
    )
}

impl<'a> QuotedLiteral<'a> {
    /// Does the body hold an interpolation span its dialect reads?
    fn interpolates(&self) -> bool {
        self.pieces().is_none_or(|p| p.contains(&Piece::Dyn))
    }

    /// The body as literal text and dynamic pieces. `None` when a span never
    /// closes: the literal is unreadable.
    fn pieces(&self) -> Option<Vec<Piece<'a>>> {
        let body = self.body;
        let b = body.as_bytes();
        let mut out = Vec::new();
        let mut lit = 0usize;
        let mut i = 0usize;
        // Every index `lit` / `i` takes sits right after an ASCII byte, or at
        // 0 / the body's end, so each `get` lands on a char boundary.
        let flush = |out: &mut Vec<Piece<'a>>, from: usize, to: usize| {
            if let Some(s) = body.get(from..to).filter(|s| !s.is_empty()) {
                out.push(Piece::Lit(s));
            }
        };
        while i < b.len() {
            let c = b[i];
            if c == b'\\' {
                i += 2;
                continue;
            }
            let span = match (self.interp, c, b.get(i + 1)) {
                (Interp::Brace, b'{', Some(b'{')) | (Interp::Brace, b'}', Some(b'}')) => {
                    // `{{` / `}}`: one literal brace.
                    flush(&mut out, lit, i + 1);
                    i += 2;
                    lit = i;
                    continue;
                }
                (Interp::Brace, b'{', _) => Some(close_brace(b, i + 1)?),
                (Interp::Template | Interp::Dollar, b'$', Some(b'{')) => {
                    Some(close_brace(b, i + 2)?)
                }
                (Interp::Dollar, b'#', Some(b'{')) if self.quote == b'"' => {
                    Some(close_brace(b, i + 2)?)
                }
                (Interp::Dollar, b'$', Some(n)) if n.is_ascii_alphabetic() || *n == b'_' => {
                    let mut j = i + 1;
                    while matches!(b.get(j), Some(c) if c.is_ascii_alphanumeric() || *c == b'_') {
                        j += 1;
                    }
                    let name = body.get(i + 1..j).unwrap_or("");
                    if name.bytes().any(|c| c.is_ascii_lowercase()) {
                        Some(j)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            match span {
                Some(end) => {
                    flush(&mut out, lit, i);
                    out.push(Piece::Dyn);
                    i = end;
                    lit = end;
                }
                None => i += 1,
            }
        }
        flush(&mut out, lit, b.len().max(lit));
        Some(out)
    }
}

/// DT.2c: the index just past the `}` closing a span whose body starts at
/// `from` (nested `{}` counted), or `None` when it never closes.
fn close_brace(b: &[u8], from: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = from;
    while let Some(&c) = b.get(i) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// DT.2c: the subject `pieces` spell, each dynamic piece the token `*`, or
/// `None` (dynamic) when a dynamic piece is not a WHOLE dot-delimited token
/// (preceded by `.` or the start, followed by `.` or the end:
/// `"orders-" + region` has no token of its own), when no literal token
/// remains (`PREFIX + "." + id`), when a token is empty or holds whitespace
/// (no NATS subject does), or when the subject fails [`fold_topic`]'s
/// length gate. A literal text holding `*` or `>` is no publish subject a
/// fold may read, so it is `None` too.
fn subject_from_pieces(pieces: &[Piece<'_>]) -> Option<String> {
    let mut out = String::new();
    for (k, p) in pieces.iter().enumerate() {
        match p {
            Piece::Lit(s) => {
                if s.contains(['*', '>']) {
                    return None;
                }
                out.push_str(s);
            }
            Piece::Dyn => {
                let before = out.is_empty() || out.ends_with('.');
                let after = match pieces.get(k + 1) {
                    None => true,
                    Some(Piece::Lit(s)) => s.starts_with('.'),
                    Some(Piece::Dyn) => false,
                };
                if !before || !after {
                    return None;
                }
                out.push_str(SUBJECT_PLACEHOLDER);
            }
        }
    }
    let tokens: Vec<&str> = out.split('.').collect();
    if tokens
        .iter()
        .any(|t| t.is_empty() || t.contains(|c: char| c.is_whitespace()))
        || tokens.iter().all(|t| *t == SUBJECT_PLACEHOLDER)
    {
        return None;
    }
    let (topic, _) = fold_topic(&out)?;
    (topic == out).then_some(topic)
}

/// DT.2c: the subject a `fmt.Sprintf` format spells ([`subject_from_pieces`]):
/// `%s %d %v %x %X %o %b %t` are dynamic pieces and `%%` a literal `%`; any
/// other verb (`%q`), a flag or a width (`%05d`, `%-8s`) or an explicit
/// argument index is `None`, so the call is dynamic. `shop.orders.%s.placed`
/// -> `shop.orders.*.placed`. Also the LA.4 fold's reading of a format held
/// in a constant (`queues::read_hit`).
pub fn subject_from_format(fmt: &str) -> Option<String> {
    let b = fmt.as_bytes();
    let mut pieces = Vec::new();
    let mut lit = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'%' {
            i += 1;
            continue;
        }
        // `%` is ASCII, so `lit..i` and `i + 2` are char boundaries.
        if let Some(s) = fmt.get(lit..i).filter(|s| !s.is_empty()) {
            pieces.push(Piece::Lit(s));
        }
        match b.get(i + 1) {
            Some(b'%') => pieces.push(Piece::Lit("%")),
            Some(b's' | b'd' | b'v' | b'x' | b'X' | b'o' | b'b' | b't') => pieces.push(Piece::Dyn),
            _ => return None,
        }
        i += 2;
        lit = i;
    }
    if let Some(s) = fmt.get(lit..).filter(|s| !s.is_empty()) {
        pieces.push(Piece::Lit(s));
    }
    subject_from_pieces(&pieces)
}

/// DT.2c: the argument region of `arg` when `arg` is exactly one call of
/// `callee` (`fmt.Sprintf("x.%s", id)`): the callee, `(`, a balanced region
/// ([`region_body`]) and its `)` as the last byte.
fn call_args<'a>(arg: &'a str, callee: &str) -> Option<&'a str> {
    let open = arg.strip_prefix(callee)?.strip_prefix('(')?;
    let inner = region_body(open)?;
    (open.get(inner.len()..) == Some(")")).then_some(inner)
}

/// DT.2c: the callee of `arg` when `arg` is exactly one call of a BARE
/// identifier (`shipment(id)`, never `p.subject(id)` or `fmt.Sprintf(..)`).
fn bare_callee(arg: &str) -> Option<&str> {
    let b = arg.as_bytes();
    if !matches!(b.first(), Some(c) if c.is_ascii_alphabetic() || *c == b'_') {
        return None;
    }
    let mut i = 1usize;
    while matches!(b.get(i), Some(c) if c.is_ascii_alphanumeric() || *c == b'_') {
        i += 1;
    }
    let name = arg.get(..i)?;
    call_args(arg, name)?;
    Some(name)
}

/// DT.2c: the returned expression of the same-file TOP-LEVEL Go function
/// `func <name>(..) string { return <expr> }` (a line opening with `func `,
/// so never a method, whose receiver comes first), when its body is exactly
/// that one statement, on one line or several, `//` comments dropped. A
/// named or multiple result, a missing body, or a body with any other
/// statement (quokka's `roomSubject` and its error branches) is `None`. The
/// first `func <name>(` decides: Go declares one per package.
fn go_single_return(source: &str, name: &str) -> Option<String> {
    let b = source.as_bytes();
    for (at, _) in source.match_indices("func ") {
        if at > 0 && b.get(at - 1) != Some(&b'\n') {
            continue;
        }
        let rest = source.get(at + "func ".len()..)?.trim_start_matches([' ', '\t']);
        let Some(params) = rest
            .strip_prefix(name)
            .map(|r| r.trim_start_matches([' ', '\t']))
            .and_then(|r| r.strip_prefix('('))
        else {
            continue;
        };
        let inner = region_body(params)?;
        let result = params.get(inner.len()..)?.strip_prefix(')')?;
        let open = result
            .trim_start_matches([' ', '\t'])
            .strip_prefix("string")?
            .trim_start_matches([' ', '\t'])
            .strip_prefix('{')?;
        let body = region_body(open)?;
        if !open.get(body.len()..)?.starts_with('}') {
            return None;
        }
        return single_return(body);
    }
    None
}

/// DT.2c: `<expr>` when `body` (a function body, without its braces) is
/// exactly `return <expr>`. A depth-0 `;` with more after it, or a line
/// break after a complete operand (Go inserts a `;` there) with more after
/// it, is a second statement.
fn single_return(body: &str) -> Option<String> {
    let text = strip_line_comments(body);
    let expr = text.trim().strip_prefix("return")?;
    if !expr.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let expr = expr.trim();
    let b = expr.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' && q != b'`' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'{' | b'[' => depth += 1,
                b')' | b'}' | b']' => depth -= 1,
                b';' if depth == 0 && !expr.get(i + 1..)?.trim().is_empty() => return None,
                b'\n' if depth == 0 => {
                    let line = expr.get(..i)?.trim_end();
                    let continues = line.ends_with([
                        '+', '-', '*', '/', '%', '&', '|', '^', '<', '>', '=', '!', ',', '(', '[',
                        '{', '.',
                    ]);
                    if !continues && !expr.get(i + 1..)?.trim().is_empty() {
                        return None;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    Some(expr.trim_end_matches(';').trim_end().to_string())
}

/// DT.2c: `s` with every `//` line comment outside a quoted literal dropped
/// (the line break kept).
fn strip_line_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut quote: Option<u8> = None;
    let mut from = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' && q != b'`' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else if matches!(c, b'\'' | b'"' | b'`') {
            quote = Some(c);
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            out.push_str(s.get(from..i).unwrap_or(""));
            // Skip to the line break, which stays.
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            from = i;
            continue;
        }
        i += 1;
    }
    out.push_str(s.get(from.min(s.len())..).unwrap_or(""));
    out
}

/// DT.2c: one [`TopicRule::NatsSubject`] / [`TopicRule::NatsFilter`]
/// occurrence: the slot [`nats_subject_slot`] picks in the call's argument
/// region, read by [`nats_subject`] / [`nats_filter`]. No region (a needle
/// that prefix-matches `nc.PublishMsg(`) or an empty slot reads nothing and
/// is not dynamic: there is no subject to judge.
fn nats_hit(source: &str, after: usize, needle: &str, rule: TopicRule, offset: usize) -> TopicHit {
    let mut hit = TopicHit {
        topic: None,
        form: TopicForm::Literal,
        offset,
        expr: None,
        dynamic: false,
        format: false,
        fold: None,
    };
    let Some(region) = arg_region(source, after, needle) else {
        return hit;
    };
    let arg = split_args(region)
        .get(nats_subject_slot(region))
        .map_or("", |a| a.trim());
    if arg.is_empty() {
        return hit;
    }
    let read = match rule {
        TopicRule::NatsFilter => nats_filter(arg),
        _ => nats_subject(arg, source),
    };
    match read {
        SubjectRead::Literal((topic, form)) => {
            hit.topic = Some(topic);
            hit.form = form;
        }
        SubjectRead::Folded { topic, how } => {
            hit.topic = Some(topic);
            hit.fold = Some(how);
        }
        SubjectRead::Expr { chain, format } => {
            hit.expr = Some(chain);
            hit.format = format;
        }
        SubjectRead::Dynamic => hit.dynamic = true,
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(source: &str, needle: &str, rule: TopicRule) -> Option<String> {
        scan(source, needle, rule).into_iter().next()?.topic
    }

    #[test]
    fn every_occurrence_is_scanned() {
        let src = "nc.Publish(\"orders\", a)\nnc.Publish(\"payments\", b)\n";
        let hits = scan(src, "nc.Publish", TopicRule::ArgLiteral);
        let topics: Vec<_> = hits.into_iter().filter_map(|h| h.topic).collect();
        assert_eq!(topics, vec!["orders".to_string(), "payments".to_string()]);
    }

    #[test]
    fn hits_are_bounded() {
        let src = "nc.Publish(\"t\", x)\n".repeat(MAX_HITS_PER_NEEDLE + 20);
        assert_eq!(
            scan(&src, "nc.Publish", TopicRule::ArgLiteral).len(),
            MAX_HITS_PER_NEEDLE
        );
    }

    #[test]
    fn scan_guarded_drops_before_the_cap() {
        let table = ".send(\"x\")\n".repeat(MAX_HITS_PER_NEEDLE + 1);
        let src = format!(
            "const TABLE: &str = r#\"\n{table}\"#;\nfn f() {{ producer.send(\"orders\", m); }}\n"
        );
        let real = src.rfind(".send(").expect("the code site");
        let mut guard = LazyGuard::new("src/table.rs", &src);
        let hits = scan_guarded(&src, ".send(", TopicRule::ArgLiteral, &mut guard);
        let read: Vec<_> = hits.iter().map(|h| (h.offset, h.topic.clone())).collect();
        assert_eq!(read, vec![(real, Some("orders".to_string()))]);
        // Unguarded, the 33 literal occurrences fill the cap and lose it.
        let plain = scan(&src, ".send(", TopicRule::ArgLiteral);
        assert_eq!(plain.len(), MAX_HITS_PER_NEEDLE);
        assert!(plain.iter().all(|h| h.offset != real));
    }

    #[test]
    fn bare_word_needles_are_whole_words() {
        let at = |src: &str, needle: &str, rule| {
            bare_word_ok(src, src.find(needle).unwrap(), needle, rule)
        };
        assert!(at(
            "HardWorker.perform_async(1)",
            "perform_async",
            TopicRule::Receiver
        ));
        assert!(at("perform_async(1)", "perform_async", TopicRule::Receiver));
        assert!(!at(
            "fn sidekiq_perform_async_x()",
            "perform_async",
            TopicRule::Receiver
        ));
        assert!(!at(
            "MyJob.perform_inline(1)",
            "perform_in",
            TopicRule::Receiver
        ));
        // Rows whose needle opens on punctuation, and every other rule, keep
        // matching inside a longer receiver name.
        assert!(at("my_producer.send(x)", ".send(", TopicRule::Receiver));
        assert!(at("xperform_async", "perform_async", TopicRule::NoIdentity));
    }

    #[test]
    fn object_form_needs_the_keyed_rule() {
        let src = "producer.send({ topic: 'orders', messages: [{ value: 'hi' }] })";
        assert_eq!(
            one(src, "producer.send", TopicRule::Keyed(&["topic"])),
            Some("orders".to_string())
        );
    }

    #[test]
    fn go_struct_field_and_word_boundary() {
        let src =
            r#"writer.WriteMessages(ctx, kafka.Message{TopicPartition: tp, Topic: "orders"})"#;
        assert_eq!(
            one(src, "writer.WriteMessages", TopicRule::Keyed(&["topic"])),
            Some("orders".to_string())
        );
    }

    #[test]
    fn php_fat_arrow_and_quoted_key() {
        let src = "$q->push(['topic' => 'orders', 'body' => $b]);";
        assert_eq!(
            one(src, "$q->push", TopicRule::Keyed(&["topic"])),
            Some("orders".to_string())
        );
    }

    #[test]
    fn ruby_symbol_value() {
        let src = "Publisher.publish(queue: :orders, payload: p)";
        assert_eq!(
            one(src, "Publisher.publish", TopicRule::Keyed(&["queue"])),
            Some("orders".to_string())
        );
    }

    #[test]
    fn backtick_literal() {
        let src = "queue.add(`emails`, job)";
        assert_eq!(
            one(src, "queue.add(", TopicRule::ArgLiteral),
            Some("emails".to_string())
        );
    }

    #[test]
    fn nested_call_in_arg_zero() {
        let src = r#"consumer.subscribe(Arrays.asList("orders"));"#;
        assert_eq!(
            one(src, "consumer.subscribe", TopicRule::ArgLiteral),
            Some("orders".to_string())
        );
    }

    #[test]
    fn arg_index_reaches_the_second_positional() {
        let src = r#"channel.basic_publish("exchange", "orders", body)"#;
        assert_eq!(
            one(src, "channel.basic_publish", TopicRule::ArgIndex(1)),
            Some("orders".to_string())
        );
    }

    #[test]
    fn arg_index_or_falls_back_only_on_an_empty_literal() {
        let rule = TopicRule::ArgIndexOr(1, 0);
        let read = |src: &str| one(src, "channel.publish", rule);
        // The routing key names the queue.
        assert_eq!(
            read("channel.publish('shop', 'orders', buf)"),
            Some("orders".into())
        );
        // An empty key (fanout, every quote style) falls back to the exchange.
        assert_eq!(
            read("channel.publish('logs', '', buf)"),
            Some("logs".into())
        );
        assert_eq!(
            read("channel.publish(\"logs\", \"\", buf)"),
            Some("logs".into())
        );
        assert_eq!(
            read("channel.publish(`logs`, ``, buf)"),
            Some("logs".into())
        );
        // A key that is not a literal never falls back to the exchange: the
        // exchange is not the queue, so no topic is read and the key's
        // expression is what the const fold sees.
        assert_eq!(read("channel.publish('shop', key, buf)"), None);
        let hits = scan("channel.publish('shop', key, buf)", "channel.publish", rule);
        assert_eq!(hits[0].expr.as_deref(), Some("key"));
        // A missing argument N is not an empty literal either.
        assert_eq!(read("channel.publish('shop')"), None);
        // The expression mirrors the fallback: the exchange's, under an empty key.
        let hits = scan(
            "channel.publish(EXCHANGE, '', buf)",
            "channel.publish",
            rule,
        );
        assert_eq!(hits[0].topic, None);
        assert_eq!(hits[0].expr.as_deref(), Some("EXCHANGE"));
        // `ArgIndex(1)` itself is unchanged: an empty literal is no topic.
        assert_eq!(
            one(
                "channel.publish('logs', '', buf)",
                "channel.publish",
                TopicRule::ArgIndex(1)
            ),
            None
        );
    }

    #[test]
    fn payload_literal_in_arg_one_is_not_a_topic() {
        let src = r#"producer.send(topicVar, "payload")"#;
        assert_eq!(one(src, "producer.send", TopicRule::ArgLiteral), None);
    }

    #[test]
    fn no_identity_rule_never_reads_a_topic() {
        let src = r#"nc.Publish("orders", a)"#;
        assert_eq!(one(src, "nc.Publish", TopicRule::NoIdentity), None);
    }

    #[test]
    fn unterminated_region_does_not_panic() {
        let src = "producer.send({ topic: 'orders'";
        assert_eq!(
            one(src, "producer.send", TopicRule::Keyed(&["topic"])),
            Some("orders".to_string())
        );
    }

    #[test]
    fn multibyte_region_does_not_panic() {
        let src = "nc.Publish(\"ünïcødé-tøpic\", 日本語のペイロード)";
        assert_eq!(
            one(src, "nc.Publish", TopicRule::ArgLiteral),
            Some("ünïcødé-tøpic".to_string())
        );
    }

    #[test]
    fn oversized_literal_is_rejected() {
        let long = "x".repeat(MAX_TOPIC_LEN + 1);
        let src = format!("nc.Publish(\"{long}\", a)");
        assert_eq!(one(&src, "nc.Publish", TopicRule::ArgLiteral), None);
    }

    #[test]
    fn keyed_or_arg_falls_back_to_positional() {
        let src = r#"consumer.subscribe("orders")"#;
        assert_eq!(
            one(src, "consumer.subscribe", TopicRule::KeyedOrArg(&["topic"])),
            Some("orders".to_string())
        );
    }

    // ---- A2.5: task-queue identity ------------------------------------

    #[test]
    fn receiver_is_the_task_not_the_payload() {
        let src = "send_email.delay(\"welcome@example.com\")";
        assert_eq!(
            one(src, ".delay(", TopicRule::Receiver),
            Some("send_email".into())
        );
        // the payload literal must be nowhere near the answer
        assert_eq!(
            one(src, ".delay(", TopicRule::ArgLiteral),
            Some("welcome@example.com".into())
        );
    }

    #[test]
    fn receiver_keeps_only_the_last_chain_segment() {
        let src = "await app.tasks.send_email.apply_async(args=[1])";
        assert_eq!(
            one(src, ".apply_async(", TopicRule::Receiver),
            Some("send_email".into())
        );
    }

    #[test]
    fn receiver_before_a_bare_needle_reads_the_class() {
        let src = "HardWorker.perform_async(order.id)";
        assert_eq!(
            one(src, "perform_async", TopicRule::Receiver),
            Some("HardWorker".into())
        );
    }

    #[test]
    fn receiver_with_nothing_before_it_is_none() {
        // Honest miss -> the caller falls through to the unresolved sentinel.
        assert_eq!(
            one("  perform_async(1)", "perform_async", TopicRule::Receiver),
            None
        );
        assert_eq!(
            one("return .delay(1)", ".delay(", TopicRule::Receiver),
            None
        );
    }

    #[test]
    fn declared_symbol_skips_a_decorator_stack() {
        let src =
            "@shared_task\n@retry(max_retries=3)\n@wraps(f)\ndef send_email(address):\n    pass\n";
        assert_eq!(
            one(src, "@shared_task", TopicRule::DeclaredSymbol),
            Some("send_email".into())
        );
    }

    #[test]
    fn declared_symbol_reads_the_name_before_the_parameter_list() {
        let src = "@dramatiq.actor\npublic async Task HandleAsync(Order o)\n";
        assert_eq!(
            one(src, "@dramatiq.actor", TopicRule::DeclaredSymbol),
            Some("HandleAsync".into())
        );
    }

    #[test]
    fn declared_symbol_gives_up_rather_than_guessing() {
        let src = "@shared_task\nx = 1\ny = 2\nz = 3\nw = 4\nv = 5\ndef send_email(a):\n";
        assert_eq!(one(src, "@shared_task", TopicRule::DeclaredSymbol), None);
    }

    #[test]
    fn enclosing_symbol_finds_the_class_above() {
        let src = "class HardWorker < ApplicationJob\n  include Sidekiq::Worker\n  def perform; end\nend\n";
        assert_eq!(
            one(src, "include Sidekiq::Worker", TopicRule::EnclosingSymbol),
            Some("HardWorker".into())
        );
    }

    #[test]
    fn enclosing_symbol_reads_an_elixir_defmodule_tail() {
        let src = "defmodule MyApp.EmailWorker do\n  use Oban.Worker\nend\n";
        assert_eq!(
            one(src, "use Oban.Worker", TopicRule::EnclosingSymbol),
            Some("EmailWorker".into())
        );
    }

    #[test]
    fn enclosing_symbol_without_a_scope_is_none() {
        assert_eq!(
            one(
                "include Sidekiq::Worker\n",
                "include Sidekiq::Worker",
                TopicRule::EnclosingSymbol
            ),
            None
        );
    }

    #[test]
    fn arg_receiver_steps_over_the_constructor() {
        let src = "Oban.insert(EmailWorker.new(%{to: \"a@b.com\"}))";
        assert_eq!(
            one(src, "Oban.insert", TopicRule::ArgReceiver),
            Some("EmailWorker".into())
        );
        // ArgLiteral is what used to run here: a payload minted as a topic.
        assert_eq!(
            one(src, "Oban.insert", TopicRule::ArgLiteral),
            Some("a@b.com".into())
        );
    }

    #[test]
    fn keyed_falls_back_to_the_line_for_paren_less_calls() {
        let src = "class HardWorker\n  sidekiq_options queue: 'critical', retry: 3\nend\n";
        assert_eq!(
            one(src, "sidekiq_options", TopicRule::Keyed(&["queue"])),
            Some("critical".into())
        );
    }

    #[test]
    fn keyed_line_fallback_does_not_reach_the_next_line() {
        let src = "sidekiq_options\nqueue: 'critical'\n";
        assert_eq!(
            one(src, "sidekiq_options", TopicRule::Keyed(&["queue"])),
            None
        );
    }

    #[test]
    fn identity_rules_never_panic_on_multibyte_or_eof() {
        for rule in [
            TopicRule::Receiver,
            TopicRule::ArgReceiver,
            TopicRule::DeclaredSymbol,
            TopicRule::EnclosingSymbol,
        ] {
            let _ = one("日本語.delay(", ".delay(", rule);
            let _ = one(".delay(", ".delay(", rule);
            let _ = one("クラス ø::delay(", ".delay(", rule);
        }
    }

    #[test]
    fn clip_never_splits_a_char() {
        assert_eq!(clip("日本語", 4), "日");
        assert_eq!(clip("abc", 99), "abc");
    }

    // ---- A2.6: URL / ARN / resource-path folding -----------------------

    #[test]
    fn normalise_topic_leaves_plain_names_alone() {
        // Every file-local broker's topic must come through byte-identical —
        // including the MQTT `/` form and a colon that is not an ARN.
        for plain in [
            "orders",
            "user-events",
            "orders.fifo",
            "sensors/temp",
            "sensors/+/temp",
            "arn:not-enough:fields",
            "http",
            "projects",
            "projects/acme/sensors/temp",
            "projects/acme/topics/orders/extra",
            "ünïcødé-tøpic",
        ] {
            assert_eq!(normalise_topic(plain), Some(plain.to_string()), "{plain}");
            assert_eq!(
                fold_topic(plain).map(|f| f.1),
                Some(TopicForm::Literal),
                "{plain}"
            );
        }
        assert_eq!(normalise_topic("  'orders', "), Some("orders".to_string()));
        // Trimmed exactly ONCE, as before A2.6: a second pass would strip the
        // `)` that the first pass exposed and silently rename an existing node.
        assert_eq!(normalise_topic("x) \""), Some("x)".to_string()));
    }

    #[test]
    fn cloud_identities_fold_to_the_name() {
        let cases = [
            (
                "https://sqs.us-east-1.amazonaws.com/123456789012/orders",
                "orders",
                TopicForm::Url,
            ),
            (
                "http://localhost:4566/000000000000/orders",
                "orders",
                TopicForm::Url,
            ),
            (
                "https://sqs.us-east-1.amazonaws.com/1/orders.fifo?x=1#f",
                "orders.fifo",
                TopicForm::Url,
            ),
            (
                "https://sqs.us-east-1.amazonaws.com/1/orders/",
                "orders",
                TopicForm::Url,
            ),
            (
                "arn:aws:sns:us-east-1:123456789012:orders",
                "orders",
                TopicForm::Arn,
            ),
            (
                "arn:aws-cn:sqs:cn-north-1:123456789012:orders.fifo",
                "orders.fifo",
                TopicForm::Arn,
            ),
            (
                "projects/my-project/topics/orders",
                "orders",
                TopicForm::Path,
            ),
            (
                "projects/my-project/subscriptions/orders-worker",
                "orders-worker",
                TopicForm::Path,
            ),
        ];
        for (raw, name, form) in cases {
            assert_eq!(fold_topic(raw), Some((name.to_string(), form)), "{raw}");
        }
        // A URL and an ARN for the same queue share one join key.
        assert_eq!(
            normalise_topic("https://sqs.us-east-1.amazonaws.com/123456789012/orders"),
            normalise_topic("arn:aws:sqs:us-east-1:123456789012:orders"),
        );
    }

    #[test]
    fn a_cloud_shape_without_a_name_is_rejected() {
        for raw in [
            "https://sqs.us-east-1.amazonaws.com",
            "https://sqs.us-east-1.amazonaws.com/",
            "arn:aws:sns:us-east-1:123456789012:",
            "https://sqs.us-east-1.amazonaws.com/1/${QUEUE}",
            "https://sqs.us-east-1.amazonaws.com/1/{queue}",
            "projects/my-project/topics/{topic}",
        ] {
            assert_eq!(normalise_topic(raw), None, "{raw}");
        }
    }

    #[test]
    fn the_fold_runs_before_the_length_gate() {
        let host = "h".repeat(MAX_TOPIC_LEN);
        let url = format!("https://{host}.example.com/123/orders");
        assert!(url.len() > MAX_TOPIC_LEN);
        assert_eq!(normalise_topic(&url), Some("orders".to_string()));
    }

    #[test]
    fn scan_reports_the_form_it_folded() {
        let src = r#"sqs.send_message(QueueUrl="https://sqs.us-east-1.amazonaws.com/1/orders")"#;
        let hit = scan(src, ".send_message(", TopicRule::Keyed(&["queueurl"])).remove(0);
        assert_eq!(hit.topic.as_deref(), Some("orders"));
        assert_eq!(hit.form, TopicForm::Url);
        let plain = scan(
            r#"nc.Publish("orders", a)"#,
            "nc.Publish",
            TopicRule::ArgLiteral,
        )
        .remove(0);
        assert_eq!(plain.form, TopicForm::Literal);
    }

    #[test]
    fn keyed_reads_a_builder_method_only_after_a_dot() {
        let src =
            r#"sendMessage(SendMessageRequest.builder().queueUrl("https://sqs/1/orders").build())"#;
        assert_eq!(
            one(src, "sendMessage(", TopicRule::Keyed(&["queueurl"])),
            Some("orders".into())
        );
        // A bare call is not a key/value pair.
        let bare = r#"send(topic("orders"))"#;
        assert_eq!(one(bare, "send(", TopicRule::Keyed(&["topic"])), None);
    }

    #[test]
    fn keyed_steps_over_a_string_prefix() {
        let py = r#"send_message(QueueUrl=f"https://sqs.{region}.amazonaws.com/{acct}/orders")"#;
        assert_eq!(
            one(py, "send_message(", TopicRule::Keyed(&["queueurl"])),
            Some("orders".into())
        );
        let cs = r#"SendMessageAsync(new SendMessageRequest { QueueUrl = $"https://sqs/{acct}/orders" })"#;
        assert_eq!(
            one(cs, "SendMessageAsync(", TopicRule::Keyed(&["queueurl"])),
            Some("orders".into())
        );
        // A bare identifier value is still not a literal.
        let var = r#"send_message(QueueUrl=f, Body="x")"#;
        assert_eq!(
            one(var, "send_message(", TopicRule::Keyed(&["queueurl"])),
            None
        );
    }

    fn expr(source: &str, needle: &str, rule: TopicRule) -> Option<String> {
        scan(source, needle, rule).into_iter().next()?.expr
    }

    #[test]
    fn expr_is_captured_only_for_identifier_slots() {
        let keyed = TopicRule::KeyedOrArg(&["topic"]);
        // Identifier chains in the topic slot, every rule shape.
        let cases: &[(&str, &str, TopicRule, &str)] = &[
            (
                "producer.send({ topic: ORDERS_TOPIC, messages })",
                "producer.send",
                keyed,
                "ORDERS_TOPIC",
            ),
            (
                "producer.send({ topic: Topics.PAYMENTS })",
                "producer.send",
                keyed,
                "Topics.PAYMENTS",
            ),
            (
                "producer.send({ topic, messages: [m] })",
                "producer.send",
                keyed,
                "topic",
            ),
            (
                "producer.send({ topic: this.topic })",
                "producer.send",
                keyed,
                "this.topic",
            ),
            (
                "kafkaTemplate.send(TOPIC, payload)",
                "Template.send(",
                TopicRule::ArgLiteral,
                "TOPIC",
            ),
            (
                "nc.Publish(SubjectOrders, data)",
                "nc.Publish",
                TopicRule::ArgLiteral,
                "SubjectOrders",
            ),
            (
                "@KafkaListener(topics = Topics.ORDERS, groupId = \"billing\")",
                "@KafkaListener",
                TopicRule::Keyed(&["topics", "topic"]),
                "Topics.ORDERS",
            ),
            (
                "ch.basic_publish(EXCHANGE, ROUTING_KEY, b)",
                "ch.basic_publish",
                TopicRule::ArgIndex(1),
                "ROUTING_KEY",
            ),
            (
                "$q->push(['topic' => $this->topic])",
                "$q->push",
                TopicRule::Keyed(&["topic"]),
                "$this->topic",
            ),
            (
                "send(topic: config?.topic())",
                "send",
                TopicRule::Keyed(&["topic"]),
                "config?.topic()",
            ),
        ];
        for (src, needle, rule, want) in cases {
            let hit = scan(src, needle, *rule)
                .into_iter()
                .next()
                .expect("needle hit");
            assert_eq!(hit.topic, None, "{src}");
            assert_eq!(hit.expr.as_deref(), Some(*want), "{src}");
        }
        // Not a plain identifier chain, or no slot at all: nothing recorded.
        let none: &[(&str, &str, TopicRule)] = &[
            (
                "producer.send({ topic: PREFIX + suffix })",
                "producer.send",
                keyed,
            ),
            (
                "producer.send({ topic: getTopic(a) })",
                "producer.send",
                keyed,
            ),
            (
                "producer.send({ topic: topics[0] })",
                "producer.send",
                keyed,
            ),
            (
                "nc.Publish(prefix + suffix, data)",
                "nc.Publish",
                TopicRule::ArgLiteral,
            ),
            (
                "nc.Publish(topicFor(a), data)",
                "nc.Publish",
                TopicRule::ArgLiteral,
            ),
            (
                "nc.Publish([TOPIC], data)",
                "nc.Publish",
                TopicRule::ArgLiteral,
            ),
            // a bare positional argument is not a shorthand member
            (
                "q.push(a, topic, b)",
                "q.push",
                TopicRule::Keyed(&["topic"]),
            ),
            ("r.ReadMessage(ctx)", "r.ReadMessage", TopicRule::NoIdentity),
            (
                "tasks.send_email.delay(user)",
                ".delay(",
                TopicRule::Receiver,
            ),
        ];
        for (src, needle, rule) in none {
            assert_eq!(expr(src, needle, *rule), None, "{src}");
        }
        // A literal read leaves no expression behind.
        let lit = scan("producer.send({ topic: 'orders' })", "producer.send", keyed);
        assert_eq!(lit[0].topic.as_deref(), Some("orders"));
        assert_eq!(lit[0].expr, None);
        // A template literal is never an identifier chain.
        assert_eq!(arg_expr("`orders.${env}`, payload", 0), None);
        assert_eq!(keyed_expr("{ topic: `orders.${env}` }", &["topic"]), None);
        // Over-long chains are refused like an over-long literal.
        let long = format!("nc.Publish({}, d)", "A".repeat(MAX_TOPIC_LEN + 1));
        assert_eq!(expr(&long, "nc.Publish", TopicRule::ArgLiteral), None);
        // Multibyte input around the slot never panics.
        assert_eq!(
            expr("nc.Publish(é, d)", "nc.Publish", TopicRule::ArgLiteral),
            None
        );
        assert_eq!(
            expr("send({ topic: ü })", "send", TopicRule::Keyed(&["topic"])),
            None
        );
    }

    // ---- CL.2: a keyed value written through a pointer helper -------------

    #[test]
    fn keyed_literal_steps_into_aws_string() {
        // aws-sdk-go's `*string` fields: the literal sits inside `aws.String(`.
        let region = "ctx, &sqs.SendMessageInput{\n\t\tQueueUrl:    aws.String(\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\"),\n\t\tMessageBody: aws.String(body),\n\t}";
        assert_eq!(
            keyed_literal(region, &["queueurl"]),
            Some(("orders".to_string(), TopicForm::Url))
        );
        // Whitespace inside the helper, and an SNS ARN.
        assert_eq!(
            keyed_literal(
                "&sns.PublishInput{TopicArn: aws.String( \"arn:aws:sns:us-east-1:1:orders\" )}",
                &["topicarn"]
            ),
            Some(("orders".to_string(), TopicForm::Arn))
        );
        // End to end through the Keyed rule.
        let src = "client.SendMessage(ctx, &sqs.SendMessageInput{QueueUrl: aws.String(\"https://sqs/1/orders\")})";
        let hit = scan(src, ".SendMessage(", TopicRule::Keyed(&["queueurl"])).remove(0);
        assert_eq!(hit.topic.as_deref(), Some("orders"));
        assert_eq!(hit.form, TopicForm::Url);
        assert_eq!(hit.expr, None);
    }

    #[test]
    fn keyed_expr_steps_into_aws_string() {
        let src = "client.SendMessage(ctx, &sqs.SendMessageInput{QueueUrl: aws.String(queueURL), MessageBody: aws.String(body)})";
        let hit = scan(src, ".SendMessage(", TopicRule::Keyed(&["queueurl"])).remove(0);
        assert_eq!(hit.topic, None);
        assert_eq!(hit.expr.as_deref(), Some("queueURL"));
        assert_eq!(
            keyed_expr(
                "{QueueUrl: aws.String( cfg.OrdersQueueURL )}",
                &["queueurl"]
            )
            .as_deref(),
            Some("cfg.OrdersQueueURL")
        );
        // The helper's whole argument must be the chain.
        for value in [
            "aws.String(base + name)",
            "aws.String(urls[0])",
            "aws.String(a, b)",
        ] {
            assert_eq!(
                keyed_expr(&format!("{{QueueUrl: {value}}}"), &["queueurl"]),
                None,
                "{value}"
            );
        }
    }

    #[test]
    fn a_non_helper_call_value_is_not_read() {
        // GUARD: `os.Getenv("Q")` names an environment variable, never a
        // queue, so neither a topic `Q` nor an expression comes back.
        let src = "client.SendMessage(ctx, &sqs.SendMessageInput{QueueUrl: os.Getenv(\"Q\")})";
        let hit = scan(src, ".SendMessage(", TopicRule::Keyed(&["queueurl"])).remove(0);
        assert_eq!(hit.topic, None);
        assert_eq!(hit.expr, None);
        assert_eq!(
            keyed_literal(
                "{TopicArn: strings.TrimSpace(\"arn:aws:sns:us-east-1:1:orders\")}",
                &["topicarn"]
            ),
            None
        );
        // The helper is matched at the value, never inside a longer name.
        assert_eq!(
            keyed_literal(
                "{QueueUrl: myaws.String(\"https://sqs/1/orders\")}",
                &["queueurl"]
            ),
            None
        );
        assert_eq!(
            keyed_expr("{QueueUrl: myaws.String(u)}", &["queueurl"]),
            None
        );
    }

    // ---- CL.3: builder chains and generic type arguments --------------------

    const SB_KEYS: TopicRule = TopicRule::Chain(&["queuename", "topicname"]);

    #[test]
    fn chain_reads_a_builder_key_to_the_statement_end() {
        // Java: the chain breaks across lines, each continuation opening with
        // `.`, and the statement ends at `;`.
        let java = "ServiceBusSenderClient s = new ServiceBusClientBuilder().connectionString(c)\n    .sender()\n    .queueName(\"orders\")\n    .buildClient();\ns.sendMessage(m);\n";
        assert_eq!(one(java, ".sender()", SB_KEYS), Some("orders".into()));
        // Scala: no `;`, the statement ends at the newline.
        let scala = "  def b = new ServiceBusClientBuilder().connectionString(c).receiver().queueName(\"orders\").buildClient()\n}\n";
        assert_eq!(one(scala, ".receiver()", SB_KEYS), Some("orders".into()));
        // C#: the builder is an argument, ended by the call's `)`.
        let cs = "await client.SubscribeAsync(new MqttTopicFilterBuilder().WithTopic(\"sensors/temp\").Build());";
        assert_eq!(
            one(
                cs,
                "MqttTopicFilterBuilder()",
                TopicRule::Chain(&["withtopic"])
            ),
            Some("sensors/temp".into())
        );
        // An AWS request built on its own statement; the URL folds (A2.6).
        let aws = "ReceiveMessageRequest request = ReceiveMessageRequest.builder()\n        .queueUrl(\"https://sqs.us-east-1.amazonaws.com/1/orders\")\n        .maxNumberOfMessages(10)\n        .build();\nsqs.receiveMessage(request);\n";
        let hit = scan(
            aws,
            "ReceiveMessageRequest.builder()",
            TopicRule::Chain(&["queueurl"]),
        )
        .remove(0);
        assert_eq!(hit.topic.as_deref(), Some("orders"));
        assert_eq!(hit.form, TopicForm::Url);
        // Keys are tried in table order: a topic receiver reads its TOPIC.
        let topic =
            "b.receiver().topicName(\"orders\").subscriptionName(\"billing\").buildClient();";
        assert_eq!(one(topic, ".receiver()", SB_KEYS), Some("orders".into()));
        // A lambda's `;` and a comment line inside the chain stay inside it.
        let processor = "b.processor()\n    // the queue\n    .queueName(\"orders\")\n    .processMessage(ctx -> { log(ctx); })\n    .buildProcessorClient();";
        assert_eq!(
            one(processor, ".processor()", SB_KEYS),
            Some("orders".into())
        );
        // A key held in a constant hands its NAME to the LA.4 fold.
        let held = "b.sender().queueName(ORDERS_QUEUE).buildClient();";
        assert_eq!(
            expr(held, ".sender()", SB_KEYS),
            Some("ORDERS_QUEUE".into())
        );
        assert_eq!(one(held, ".sender()", SB_KEYS), None);
    }

    #[test]
    fn chain_stops_at_the_next_statement() {
        // A newline neither side of which is `.` ends a Scala / Kotlin chain.
        assert_eq!(
            one("a.receiver()\nfoo.queueName(\"x\")", ".receiver()", SB_KEYS),
            None
        );
        // `;` ends a Java one, `,` the argument, `)` the enclosing call.
        assert_eq!(
            one("a.receiver(); b.queueName(\"x\");", ".receiver()", SB_KEYS),
            None
        );
        assert_eq!(
            one(
                "f(a.receiver(), b.queueName(\"x\"))",
                ".receiver()",
                SB_KEYS
            ),
            None
        );
        assert_eq!(
            one(
                "f(a.receiver()) + b.queueName(\"x\")",
                ".receiver()",
                SB_KEYS
            ),
            None
        );
        // A quoted `;` is text, not a statement end.
        assert_eq!(
            one(
                "a.receiver().tag(\"a;b\").queueName(\"x\");",
                ".receiver()",
                SB_KEYS
            ),
            Some("x".into())
        );
        // A bare call is not a builder key: the `.` before the key is required.
        assert_eq!(
            one("a.receiver().f(queueName(\"x\"));", ".receiver()", SB_KEYS),
            None
        );
        // Bounded, and panic-free on EOF / multi-byte text.
        let long = format!("a.receiver(){}.queueName(\"x\");", " ".repeat(MAX_REGION));
        assert_eq!(one(&long, ".receiver()", SB_KEYS), None);
        for src in [
            "a.receiver()",
            "a.receiver().queueName(\"é",
            "a.receiver().quéueName(\"x\")",
        ] {
            let _ = scan(src, ".receiver()", SB_KEYS);
        }
        assert_eq!(chain_region("x", 5), "");
    }

    #[test]
    fn arg_region_skips_generic_arguments() {
        let at = |src: &str, needle: &str| {
            let after = src.find(needle).map(|i| i + needle.len()).unwrap_or(0);
            arg_region(src, after, needle).map(str::to_string)
        };
        assert_eq!(
            at("nc.SubscribeAsync<string>(\"orders\")", ".SubscribeAsync"),
            Some("\"orders\"".into())
        );
        // Nested arguments, `>>>` closing three levels.
        assert_eq!(
            at(
                "bus.PublishAsync<Dictionary<string, List<int>>>(\"orders\", d)",
                ".PublishAsync"
            ),
            Some("\"orders\", d".into())
        );
        // Hangfire (CL.4): the type argument, then a lambda.
        assert_eq!(
            at("BackgroundJob.Enqueue<T>(x => x.Run())", ".Enqueue"),
            Some("x => x.Run()".into())
        );
        // Unclosed, or a comparison, reads nothing.
        assert_eq!(
            at("nc.SubscribeAsync<string(\"orders\")", ".SubscribeAsync"),
            None
        );
        assert_eq!(at("if (nc.Count < 3) { go(); }", "nc.Count"), None);
        assert_eq!(at("nc.SubscribeAsync<string", ".SubscribeAsync"), None);
        // A dotless TYPE needle is not stepped over: its constructor argument
        // is client config, never a topic expression.
        assert_eq!(at("new KafkaConsumer<>(props)", "KafkaConsumer"), None);
        assert_eq!(
            expr(
                "new KafkaConsumer<>(props)",
                "KafkaConsumer",
                TopicRule::ArgLiteral
            ),
            None
        );
        assert_eq!(
            one(
                "nc.SubscribeAsync<Order>(\"orders\")",
                ".SubscribeAsync",
                TopicRule::ArgLiteral
            ),
            Some("orders".into())
        );
    }

    #[test]
    fn skip_scala_type_args_balances_and_aborts() {
        // Balanced, nested, and the rest handed back unconsumed.
        assert_eq!(skip_scala_type_args("[String, String](\"o\")"), Some("(\"o\")"));
        assert_eq!(skip_scala_type_args("[F[_], Map[K, List[V]]](x)"), Some("(x)"));
        // A function type's `=>` is no stop; a quote holding `]` is stepped over.
        assert_eq!(skip_scala_type_args("[String => Int](f)"), Some("(f)"));
        assert_eq!(skip_scala_type_args("[\"]\" , A]]"), Some("]"));
        // An empty list closes.
        assert_eq!(skip_scala_type_args("[](x)"), Some("(x)"));
        // Aborts: a `(` / `{` / `;` / newline before the close, unclosed, or
        // longer than MAX_REGION.
        assert_eq!(skip_scala_type_args("[a(1)]"), None);
        assert_eq!(skip_scala_type_args("[a{1}]"), None);
        assert_eq!(skip_scala_type_args("[a; b]"), None);
        assert_eq!(skip_scala_type_args("[String,\n String](x)"), None);
        assert_eq!(skip_scala_type_args("[String, String"), None);
        let long = format!("[{}]", "A".repeat(MAX_REGION));
        assert_eq!(skip_scala_type_args(&long), None);
        // Multi-byte text inside the list slices cleanly.
        assert_eq!(skip_scala_type_args("[Événement](x)"), Some("(x)"));
    }

    #[test]
    fn ctor_arg_literal_reads_arg0_after_type_arguments() {
        let rule = TopicRule::CtorArgLiteral;
        let topics = |src: &str| -> Vec<Option<String>> {
            scan(src, "ProducerRecord", rule).into_iter().map(|h| h.topic).collect()
        };
        let o = || vec![Some("orders".to_string())];
        // Java explicit generics, Java diamond, Scala `[..]`, Kotlin without
        // `new`, and no type arguments at all.
        assert_eq!(topics("new ProducerRecord<String, String>(\"orders\", v)"), o());
        assert_eq!(topics("new ProducerRecord<>(\"orders\", v)"), o());
        assert_eq!(topics("new ProducerRecord[String, String](\"orders\", body)"), o());
        assert_eq!(topics("val r = ProducerRecord<String, String>(\"orders\", v)"), o());
        assert_eq!(topics("new ProducerRecord(\"orders\", v)"), o());
        assert_eq!(topics("new ProducerRecord<Map<K, V>, String>(\n  \"orders\", v)"), o());
        // Spaces around the type-argument list are the constructor's too.
        assert_eq!(topics("new ProducerRecord <String, String> (\"orders\", v)"), o());
        assert_eq!(topics("new ProducerRecord[String, String] (\"orders\", v)"), o());
        // No construction: no site at all, not a site without a topic.
        for src in [
            "import org.apache.kafka.clients.producer.ProducerRecord;",
            "ProducerRecord<String, String> r = build();",
            "val c = classOf[ProducerRecord[String, String]]",
            "ProducerRecord.class",
            "new MyProducerRecord(\"orders\")",
            "List<ProducerRecords> all;",
            "ProducerRecords(\"orders\")",
            "import shop.ProducerRecord\n(\"orders\", v)",
        ] {
            assert!(scan(src, "ProducerRecord", rule).is_empty(), "{src}");
        }
        // A declaration and its constructor on one line: one site.
        assert_eq!(
            topics("ProducerRecord<String, String> r = new ProducerRecord<>(\"orders\", v);"),
            o()
        );
        // A non-literal arg #0 is a hit carrying its expression (LA.4).
        let hits = scan("new ProducerRecord<String, String>(Topics.ORDERS, v)", "ProducerRecord", rule);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].topic, None);
        assert_eq!(hits[0].expr.as_deref(), Some("Topics.ORDERS"));
        // The dotless ArgLiteral rule still does not step over the list.
        assert_eq!(one("new ProducerRecord<String, String>(\"orders\", v)", "ProducerRecord", TopicRule::ArgLiteral), None);
    }

    #[test]
    fn lambda_callee_forms() {
        let lc = |src: &str, needle: &str| one(src, needle, TopicRule::LambdaCallee);
        let s = |v: &str| Some(v.to_string());
        // Java / Scala / C# arrows; the receiver is never the job.
        assert_eq!(
            lc(
                "BackgroundJob.enqueue(() -> emailJobs.sendWelcome(id));",
                "BackgroundJob.enqueue"
            ),
            s("sendWelcome")
        );
        assert_eq!(
            lc(
                "BackgroundJob.enqueue(() => emailJobs.sendWelcome(id))",
                "BackgroundJob.enqueue"
            ),
            s("sendWelcome")
        );
        assert_eq!(
            lc(
                "BackgroundJob.Enqueue(() => Console.WriteLine(\"hi\"));",
                ".Enqueue"
            ),
            s("WriteLine")
        );
        // A generic argument before `(` (CL.3's skip), a lambda parameter.
        assert_eq!(
            lc(
                "BackgroundJob.Enqueue<EmailJobs>(x => x.SendWelcome(id));",
                ".Enqueue"
            ),
            s("SendWelcome")
        );
        assert_eq!(
            lc(
                "BackgroundJob.<EmailService>enqueue(x -> x.send(id));",
                ">enqueue("
            ),
            s("send")
        );
        // A block lambda, `return` / `await` skipped; a delay or a parent id first.
        assert_eq!(
            lc(
                "BackgroundJob.enqueue(() -> { svc.run(); });",
                "BackgroundJob.enqueue"
            ),
            s("run")
        );
        assert_eq!(
            lc(
                "BackgroundJob.Enqueue(() => { return svc.Run(); });",
                ".Enqueue"
            ),
            s("Run")
        );
        assert_eq!(
            lc("x.Enqueue(async () => await svc.RunAsync())", ".Enqueue"),
            s("RunAsync")
        );
        assert_eq!(
            lc(
                "BackgroundJob.schedule(Instant.now().plusHours(1), () -> cleaner.purge());",
                "BackgroundJob.schedule"
            ),
            s("purge")
        );
        assert_eq!(
            lc(
                "BackgroundJob.ContinueJobWith(id, () => Reports.Build(\"a->b\"));",
                ".ContinueJobWith"
            ),
            s("Build")
        );
        // An arrow inside a quoted argument or a nested call is not the lambda.
        assert_eq!(
            lc(
                "BackgroundJob.Enqueue(Make(\"a => b\"), () => J.Go())",
                ".Enqueue"
            ),
            s("Go")
        );
        // Kotlin trailing lambdas: alone, after the arguments, with a parameter.
        assert_eq!(
            lc(
                "BackgroundJob.enqueue { mailer.sendWelcome(id) }",
                "BackgroundJob.enqueue"
            ),
            s("sendWelcome")
        );
        assert_eq!(
            lc(
                "BackgroundJob.schedule(at) { mailer.remind(id) }",
                "BackgroundJob.schedule"
            ),
            s("remind")
        );
        assert_eq!(
            lc(
                "BackgroundJob.enqueue<Mailer> { it.digest() }",
                "BackgroundJob.enqueue"
            ),
            s("digest")
        );
        assert_eq!(
            lc(
                "jobScheduler.enqueue { m -> m.digest() }",
                "Scheduler.enqueue"
            ),
            s("digest")
        );
        // No lambda, no call, or a body that is not a plain call: nothing.
        assert_eq!(lc("_q.Enqueue(item);", ".Enqueue"), None);
        assert_eq!(
            lc("BackgroundJob.enqueue(svc::run);", "BackgroundJob.enqueue"),
            None
        );
        assert_eq!(
            lc("BackgroundJob.enqueue(() -> 42);", "BackgroundJob.enqueue"),
            None
        );
        assert_eq!(
            lc(
                "BackgroundJob.enqueue(() -> new Job());",
                "BackgroundJob.enqueue"
            ),
            None
        );
        assert_eq!(
            lc(
                "BackgroundJob.enqueue(() -> svc.field);",
                "BackgroundJob.enqueue"
            ),
            None
        );
        assert_eq!(
            lc(
                "BackgroundJob.schedule(at)\n{ svc.run() }",
                "BackgroundJob.schedule"
            ),
            None
        );
        assert_eq!(
            lc(
                "BackgroundJob.scheduleRecurrently(\"n\", () -> svc.run())",
                "BackgroundJob.schedule"
            ),
            None
        );
        // An identity rule: no LA.4 expression is ever recorded.
        let hit = scan("_q.Enqueue(item);", ".Enqueue", TopicRule::LambdaCallee);
        assert!(hit.iter().all(|h| h.topic.is_none() && h.expr.is_none()));
    }

    #[test]
    fn declared_symbol_reads_a_kotlin_fun_and_types_are_told_apart() {
        let src = "@Job(name = \"digest\")\nfun sendDigest(id: String) { }\n";
        assert_eq!(
            one(src, "@Job(", TopicRule::DeclaredSymbol),
            Some("sendDigest".into())
        );
        // `def` still wins over a later `fun` on the same line.
        let py = "@shared_task\ndef fun (x):\n";
        assert_eq!(
            one(py, "@shared_task", TopicRule::DeclaredSymbol),
            Some("fun".into())
        );
        let after =
            |src: &str, needle: &str| src.find(needle).map(|i| i + needle.len()).unwrap_or(0);
        for (src, is_type) in [
            (
                "[AutomaticRetry(Attempts = 0)]\npublic class EmailJobs\n{",
                true,
            ),
            (
                "[AutomaticRetry(Attempts = 0)]\npublic sealed class EmailJobs\n{",
                true,
            ),
            ("[Queue(\"q\")]\npublic record EmailJob(string Id);", true),
            ("@Job(name = \"x\")\nclass Jobs(val m: Mailer) {", true),
            (
                "[Queue(\"q\")]\npublic void SendWelcome(object state) { }",
                false,
            ),
            (
                "@Job(name = \"x\")\npublic void sendWelcome(String id) {",
                false,
            ),
            (
                "@Job(name = \"x\")\ndef sendWelcome(id: String): Unit = ()",
                false,
            ),
            ("[Queue(\"q\")]\n", false),
        ] {
            let needle = if src.starts_with('@') {
                "@Job("
            } else if src.starts_with("[Queue") {
                "[Queue("
            } else {
                "[AutomaticRetry("
            };
            assert_eq!(declares_type(src, after(src, needle)), is_type, "{src}");
        }
    }

    fn jobs(src: &str, needle: &str, rule: TopicRule) -> Vec<String> {
        scan(src, needle, rule)
            .into_iter()
            .filter_map(|h| h.topic)
            .collect()
    }

    #[test]
    fn storage_job_type_reads_the_generic_then_the_pushed_type() {
        let push = |src: &str| jobs(src, ".push(", TopicRule::StorageJobType);
        // The storage's type argument; `&mut`, the crate path and the job's
        // module prefix are stepped over.
        assert_eq!(
            push(
                "async fn e(storage: &mut apalis_redis::RedisStorage<crate::WelcomeEmail>) {\n    storage.push(crate::WelcomeEmail { to: t }).await;\n}\n"
            ),
            ["WelcomeEmail"]
        );
        // The type argument wins over the pushed value; a typed `let`, a
        // turbofish initializer and an `impl Storage<Job = ..>` read it too.
        for src in [
            "fn e(s: &mut RedisStorage<Email>, j: Email) { s.push(j); }",
            "fn e() { let mut s: SqliteStorage<Email> = SqliteStorage::new(p); s.push(j); }",
            "fn e() { let mut s = PostgresStorage::<Email>::new(pool); s.push(j); }",
            "fn e(s: &mut impl Storage<Job = Email>) { s.push(j); }",
            "fn e(s: &'a mut RedisStorage<Email, JsonCodec<Vec<u8>>>) { s.push(j); }",
        ] {
            assert_eq!(push(src), ["Email"], "{src}");
        }
        // An untyped storage: the type the pushed value is built from.
        for (value, job) in [
            ("Email { to: t }", "Email"),
            ("Email(t)", "Email"),
            ("jobs::Email::new(t)", "Email"),
            ("Email::default()", "Email"),
            ("Job::Welcome { to: t }", "Job"),
        ] {
            let src = format!(
                "fn e() {{\n    let mut storage = MemoryStorage::new();\n    storage.push({value});\n}}\n"
            );
            assert_eq!(push(&src), [job], "{value}");
        }
        // A line-broken chain still names its receiver.
        assert_eq!(
            push(
                "fn e() {\n    let mut storage = MemoryStorage::new();\n    storage\n        .push(Email { to: t })\n        .await;\n}\n"
            ),
            ["Email"]
        );
        // The nearest declaration wins: a `Vec` named like an earlier storage
        // is no storage, and a commented declaration shadows nothing.
        assert_eq!(
            push(
                "fn a(storage: &mut RedisStorage<Email>) {}\nfn b() {\n    let mut storage = Vec::new();\n    storage.push(Report { id: 1 });\n}\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            push(
                "fn e(storage: &mut RedisStorage<Email>) {\n    // storage: the shared handle\n    storage.push(x);\n}\n"
            ),
            ["Email"]
        );
        // No site: not a storage, undeclared, a generic job, a variable into
        // an untyped storage, a constant, a call-result receiver.
        for src in [
            "fn e() { let mut reports = Vec::new(); reports.push(Report { id: 1 }); }",
            "fn e() { storage.push(Email { to: t }); }",
            "fn e<J>(s: &mut RedisStorage<J>, job: J) { s.push(job); }",
            "fn e() { let mut s = MemoryStorage::new(); s.push(job); }",
            "fn e() { let mut s = MemoryStorage::new(); s.push(DEFAULT_JOB); }",
            "fn e() { storage().push(Email { to: t }); }",
        ] {
            assert!(
                scan(src, ".push(", TopicRule::StorageJobType).is_empty(),
                "{src}"
            );
        }
    }

    #[test]
    fn worker_job_type_reads_the_handler_then_the_backend() {
        let build = |src: &str, needle: &str| jobs(src, needle, TopicRule::WorkerJobType);
        // The handler's first parameter type (async or not, `self` skipped,
        // generics stepped over, a destructuring pattern's type).
        for handler in [
            "async fn send_welcome(job: WelcomeEmail) -> Result<(), Error> { Ok(()) }",
            "pub fn send_welcome<C>(job: jobs::WelcomeEmail, ctx: Data<C>) {}",
            "fn send_welcome(WelcomeEmail { to }: WelcomeEmail) {}",
        ] {
            let src = format!(
                "{handler}\n\nfn w(s: S) -> W {{\n    WorkerBuilder::new(\"w\").backend(s).build_fn(send_welcome)\n}}\n"
            );
            assert_eq!(build(&src, ".build_fn("), ["WelcomeEmail"], "{handler}");
        }
        // apalis main spells it `.build(`, the chain line-broken.
        assert_eq!(
            build(
                "async fn send_email(task: Email, data: Data<usize>) {}\n\nfn main() {\n    let w = WorkerBuilder::new(\"email-worker\")\n        .concurrency(2)\n        .backend(storage)\n        .build(send_email);\n}\n",
                ".build("
            ),
            ["Email"]
        );
        // A handler declared elsewhere: the backend storage, declared in the
        // file, written inline, or (apalis 0.4 / 0.5) `.with_storage(`, and
        // the worker registered inside an enclosing call.
        for src in [
            "fn w(storage: RedisStorage<Email>) {\n    Monitor::new().register(WorkerBuilder::new(\"w\").backend(storage.clone()).build_fn(jobs::send));\n}\n",
            "fn w() {\n    WorkerBuilder::new(\"w\").backend(RedisStorage::<Email>::new(conn)).build_fn(send);\n}\n",
            "fn w(storage: SqliteStorage<Email>) {\n    WorkerBuilder::new(\"w\").with_storage(storage).build_fn(send);\n}\n",
        ] {
            assert_eq!(build(src, ".build_fn("), ["Email"], "{src}");
        }
        // No site: no WorkerBuilder in the statement (an earlier statement's
        // does not count), or no readable job type.
        for src in [
            "fn c() { let http = reqwest::Client::builder().timeout(t).build(x); }",
            "fn c() { let w = WorkerBuilder::new(\"w\"); let h = Client::builder().build(x); }",
            "fn w(s: S) { WorkerBuilder::new(\"w\").backend(s).build_fn(elsewhere::send); }",
        ] {
            let needle = if src.contains(".build_fn(") {
                ".build_fn("
            } else {
                ".build("
            };
            assert!(
                scan(src, needle, TopicRule::WorkerJobType).is_empty(),
                "{src}"
            );
        }
        // Neither rule reads an expression, and a symbol is never a URL.
        let hit = scan(
            "fn w() { WorkerBuilder::new(\"w\").backend(RedisStorage::<Email>::new(c)).build_fn(h); }",
            ".build_fn(",
            TopicRule::WorkerJobType,
        );
        assert!(
            hit.iter()
                .all(|h| h.expr.is_none() && h.form == TopicForm::Literal)
        );
    }

    // ---- DH.1: Declared and BareArg ----------------------------------------

    fn bare(src: &str, needle: &str) -> Vec<Option<String>> {
        scan(src, needle, TopicRule::BareArg)
            .into_iter()
            .map(|h| {
                assert!(h.expr.is_none(), "BareArg never records an expression");
                h.topic
            })
            .collect()
    }

    #[test]
    fn bare_arg_reads_symbol_string_and_paren_forms() {
        let orders = vec![Some("orders".to_string())];
        // Karafka's routing DSL, paren-less: a symbol or a string.
        for src in [
            "routes.draw do\n  topic :orders do\n    consumer OrdersConsumer\n  end\nend\n",
            "  topic \"orders\" do\n    consumer OrdersConsumer\n  end\n",
            "  topic 'orders' do\n  end\n",
            "  topic \t :orders do\n  end\n",
            // Parenthesised: argument #0 must start with the literal.
            "  topic (:orders) do\n  end\n",
            "  topic (\"orders\", x) do\n  end\n",
        ] {
            assert_eq!(bare(src, "topic "), orders, "{src:?}");
        }
        // A member needle, google-cloud-pubsub's documented paren-less call.
        assert_eq!(bare("topic = pubsub.topic \"orders\"\n", ".topic "), orders);
        // Every occurrence, each its own topic.
        assert_eq!(
            bare("topic :orders do\nend\ntopic :payments do\nend\n", "topic "),
            vec![Some("orders".to_string()), Some("payments".to_string())]
        );
    }

    #[test]
    fn bare_arg_identifier_reads_nothing() {
        // No literal right after the word is no site at all: no hit, so no
        // `unresolved:` tag downstream either.
        for src in [
            "topic = message.topic\n",
            "  topic Topics::ORDERS do\n  end\n",
            "# topic name here\n",
            "  topic (Foo.new(\"orders\")) do\n  end\n",
            // A paren-less call ends with its line.
            "  topic \n    \"orders\"\n",
            "  topic ::Orders do\n  end\n",
        ] {
            assert!(
                scan(src, "topic ", TopicRule::BareArg).is_empty(),
                "{src:?}"
            );
        }
    }

    #[test]
    fn bare_arg_needs_a_word_edge() {
        assert!(scan("  my_topic :orders\n", "topic ", TopicRule::BareArg).is_empty());
        assert!(scan("  subtopic \"orders\"\n", "topic ", TopicRule::BareArg).is_empty());
        assert_eq!(
            bare("  my_topic :x\n  topic :orders do\n", "topic "),
            vec![Some("orders".to_string())]
        );
        let at = |src: &str| {
            bare_word_ok(
                src,
                src.find("topic ").unwrap(),
                "topic ",
                TopicRule::BareArg,
            )
        };
        assert!(at("topic :orders"));
        assert!(!at("subtopic :orders"));
        assert!(!at("topic orders"));
    }

    #[test]
    fn declared_reads_no_topic_and_no_expr() {
        // Bunny: `q.subscribe(..)` names no queue; even a literal argument
        // is no topic under this rule (the declaration is the identity).
        let src = "q = ch.queue(\"orders\", durable: true)\nq.subscribe(block: true) do |_, _, body|\nend\nq.subscribe(\"x\", queue)\n";
        let hits = scan(src, ".subscribe(", TopicRule::Declared);
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .all(|h| h.topic.is_none() && h.expr.is_none() && h.form == TopicForm::Literal)
        );
        assert_eq!(
            hits.iter().map(|h| h.offset).collect::<Vec<_>>(),
            src.match_indices(".subscribe(")
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        );
    }

    // ---- DH.3: CommandArg ---------------------------------------------------

    const SEND_OBJECT: TopicRule = TopicRule::CommandArg("publish");

    fn command(src: &str) -> Vec<(Option<String>, Option<String>)> {
        scan(src, ".send_object(", SEND_OBJECT)
            .into_iter()
            .map(|h| (h.topic, h.expr))
            .collect()
    }

    #[test]
    fn command_arg_reads_after_the_command() {
        // package:redis: the channel is the element right after `PUBLISH`,
        // never the payload.
        let src = "await cmd.send_object(['PUBLISH', 'orders', 'order-1']);\n";
        assert_eq!(command(src), vec![(Some("orders".to_string()), None)]);
        assert_eq!(
            command_arg("['PUBLISH', 'orders', 'x']", "publish"),
            Some(Some(("orders".to_string(), TopicForm::Literal)))
        );
        // A positional command, a typed Dart list, Redix's `conn` first and a
        // pipeline of vectors (the first publish names the hit).
        for (region, topic) in [
            ("'PUBLISH', 'orders', payload", "orders"),
            ("<Object>['PUBLISH', \"orders\", body]", "orders"),
            ("conn, [\"PUBLISH\", \"orders\", payload]", "orders"),
            (
                "conn, [[\"SET\", \"k\", \"v\"], [\"PUBLISH\", \"news\", m]]",
                "news",
            ),
        ] {
            assert_eq!(
                command_arg(region, "publish").and_then(|r| r.map(|(t, _)| t)),
                Some(topic.to_string()),
                "{region}"
            );
        }
        // Every occurrence is its own hit.
        let two = "cmd.send_object(['PUBLISH', 'a', x]);\ncmd.send_object(['PUBLISH', 'b', y]);\n";
        assert_eq!(
            command(two),
            vec![(Some("a".to_string()), None), (Some("b".to_string()), None)]
        );
    }

    #[test]
    fn command_arg_is_case_insensitive() {
        for verb in ["publish", "Publish", "PUBLISH", "pUbLiSh"] {
            let src = format!("cmd.send_object(['{verb}', 'orders', 'x']);\n");
            assert_eq!(
                command(&src),
                vec![(Some("orders".to_string()), None)],
                "{verb}"
            );
        }
        // The VERB is compared case-insensitively; the channel keeps its case.
        assert_eq!(
            command("cmd.send_object([\"PUBLISH\", \"Orders.V1\", m]);\n"),
            vec![(Some("Orders.V1".to_string()), None)]
        );
        // `PUBLISHX` is another command.
        assert!(command("cmd.send_object(['PUBLISHX', 'orders', 'x']);\n").is_empty());
    }

    #[test]
    fn command_arg_without_the_command_reads_nothing() {
        // Another command, an empty vector, no vector: no site at all.
        for src in [
            "cmd.send_object(['SET', 'k', 'v']);\n",
            "cmd.send_object(['GET', 'orders']);\n",
            "cmd.send_object([]);\n",
            "cmd.send_object(args);\n",
        ] {
            assert!(command(src).is_empty(), "{src:?}");
        }
        // The command must OPEN its vector: a key spelled `publish` is no verb.
        assert!(command("cmd.send_object(['SET', 'publish', 'v']);\n").is_empty());
        assert_eq!(command_arg("'SET', 'publish', 'v'", "publish"), None);
        // The command with no literal after it: a site, no topic, and no
        // expression for the LA.4 fold.
        assert_eq!(
            command("cmd.send_object(['PUBLISH', channel, m]);\n"),
            vec![(None, None)]
        );
        assert_eq!(command_arg("['PUBLISH']", "publish"), Some(None));
    }

    // ---- DH.4: KeyedIn and the Oban pipe ------------------------------------

    /// (topic, expr) of every KeyedIn hit, in order.
    fn keyed_in_hits(src: &str, needle: &str, keys: &'static [&'static str]) -> Vec<(Option<String>, Option<String>)> {
        scan(src, needle, TopicRule::KeyedIn(keys))
            .into_iter()
            .map(|h| (h.topic, h.expr))
            .collect()
    }

    #[test]
    fn keyed_in_reads_the_enclosing_tuple() {
        // matrix/elixir/kafka: the needle opens nothing, so the region is the
        // tuple around it; `group_id: "orders"` is another key.
        let one_line = "producer: [module: {BroadwayKafka.Producer, hosts: [localhost: 9092], group_id: \"g\", topics: [\"orders\"]}],\n";
        assert_eq!(
            keyed_in_hits(one_line, "BroadwayKafka.Producer", &["topics"]),
            vec![(Some("orders".to_string()), None)]
        );
        // The hexdocs.pm/broadway_kafka config, spread over five lines.
        let multiline = "producer: [\n  module:\n    {BroadwayKafka.Producer,\n     [hosts: [localhost: 9092],\n      group_id: \"group_1\",\n      topics: [\"orders\"]]},\n  concurrency: 1\n],\n";
        assert_eq!(
            keyed_in_hits(multiline, "BroadwayKafka.Producer", &["topics"]),
            vec![(Some("orders".to_string()), None)]
        );
        // A quoted bracket before the needle is no group, and a sibling
        // tuple's key never leaks in: each needle reads only its own group.
        let two = "children = [\n  {Other, name: \"{x\", topics: [\"audit\"]},\n  {BroadwayKafka.Producer, topics: [\"orders\"]}\n]\n";
        assert_eq!(
            keyed_in_hits(two, "BroadwayKafka.Producer", &["topics"]),
            vec![(Some("orders".to_string()), None)]
        );
        // A needle that opens its bracket reads that one (Tortoise's call).
        let call = "Tortoise311.Supervisor.start_child(client_id: \"l\", subscriptions: [{\"sensors/temp\", 0}])\n";
        assert_eq!(
            keyed_in_hits(call, "Tortoise311.Supervisor.start_child(", &["subscriptions"]),
            vec![(Some("sensors/temp".to_string()), None)]
        );
        // A GCP subscription path folds (the A2.6 rule).
        let gcp = "{BroadwayCloudPubSub.Producer, subscription: \"projects/shop/subscriptions/orders\"}";
        let hits = scan(gcp, "BroadwayCloudPubSub.Producer", TopicRule::KeyedIn(&["subscription"]));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].topic.as_deref(), Some("orders"));
        assert_eq!(hits[0].form, TopicForm::Path);
    }

    #[test]
    fn keyed_in_steps_into_a_list_of_tuples() {
        // Tortoise's subscription tuple, a list of them (the first), Gnat's
        // list of maps under the key `topic`, and a map value.
        let cases: [(&str, &str, &'static [&'static str], &str); 4] = [
            ("Tortoise.Connection.start_link(subscriptions: [{\"a/b\", 0}, {\"c/d\", 1}])", "Tortoise.Connection.start_link(", &["subscriptions"], "a/b"),
            ("{Gnat.ConsumerSupervisor, %{connection_name: :gnat, module: S, subscription_topics: [%{topic: \"rpc.example\"}]}}", "Gnat.ConsumerSupervisor", &["topic"], "rpc.example"),
            ("{X.Producer, opts: %{topics: [ \"spaced\" ]}}", "X.Producer", &["topics"], "spaced"),
            ("{X.Producer, topics: \"plain\"}", "X.Producer", &["topics"], "plain"),
        ];
        for (src, needle, keys, want) in cases {
            assert_eq!(keyed_in_hits(src, needle, keys), vec![(Some(want.to_string()), None)], "{src}");
        }
        // At most two levels: a literal three brackets deep is no value.
        assert_eq!(
            keyed_in_hits("{X.Producer, topics: [[[\"deep\"]]]}", "X.Producer", &["topics"]),
            vec![(None, None)]
        );
        // `subscription_topics` is not the key `topic`, nor `topics`.
        assert_eq!(keyed_in("subscription_topics: [\"x\"]", &["topic"]), None);
        assert_eq!(keyed_in("subscription_topics: [\"x\"]", &["topics"]), None);
    }

    #[test]
    fn keyed_in_variable_value_reads_nothing() {
        // A key whose value is a variable: a site, no topic, no expression.
        for src in [
            "{BroadwayKafka.Producer, hosts: @hosts, topics: @topics}",
            "{BroadwayKafka.Producer, topics: topics}",
            "{BroadwayKafka.Producer, topics: [topic]}",
        ] {
            assert_eq!(
                keyed_in_hits(src, "BroadwayKafka.Producer", &["topics"]),
                vec![(None, None)],
                "{src}"
            );
        }
        // No key at all, or no group around the needle: no site. A Tortoise
        // connection that subscribes to nothing is a publisher's.
        let no_site: [(&str, &str, &'static [&'static str]); 4] = [
            ("{BroadwayKafka.Producer, kafka_opts()}", "BroadwayKafka.Producer", &["topics"]),
            ("{Gnat.ConsumerSupervisor, consumer_supervisor_settings}", "Gnat.ConsumerSupervisor", &["topic"]),
            ("defmodule X do\n  alias BroadwayKafka.Producer\nend\n", "BroadwayKafka.Producer", &["topics"]),
            ("Tortoise311.Supervisor.start_child(client_id: \"pub\", server: {Tcp, host: \"b\"})", "Tortoise311.Supervisor.start_child(", &["subscriptions"]),
        ];
        for (src, needle, keys) in no_site {
            assert!(keyed_in_hits(src, needle, keys).is_empty(), "{src}");
        }
        // The walk back is bounded: a group opened more than MAX_REGION bytes
        // before the needle is never found.
        let far = format!("{{topics: [\"x\"], {}BroadwayKafka.Producer}}", " ".repeat(MAX_REGION + 1));
        assert!(keyed_in_hits(&far, "BroadwayKafka.Producer", &["topics"]).is_empty());
        // Byte walks never slice a char: multi-byte text around the needle.
        let utf8 = "{é: \"ü\", BroadwayKafka.Producer, topics: [\"ørders\"]}";
        assert_eq!(
            keyed_in_hits(utf8, "BroadwayKafka.Producer", &["topics"]),
            vec![(Some("ørders".to_string()), None)]
        );
    }

    #[test]
    fn unmatched_open_before_skips_strings_and_closed_groups() {
        let s = b"f(a, [1, 2], \"(\\\"[\", x";
        assert_eq!(unmatched_open_before(s, s.len()), Some(1));
        // An apostrophe (a `# don't` comment) does not open a string.
        let s = b"{ # don't\n  X.Producer";
        assert_eq!(unmatched_open_before(s, s.len()), Some(0));
        assert_eq!(unmatched_open_before(b"no brackets", 11), None);
        assert_eq!(unmatched_open_before(b"(x)", 3), None);
    }

    #[test]
    fn arg_receiver_reads_the_pipe_stage() {
        // matrix/elixir/taskq signup.ex: the stage on the line before.
        let signup = "defmodule MyApp.Signup do\n  def register(user_id) do\n    %{user_id: user_id}\n    |> MyApp.MailerWorker.new()\n    |> Oban.insert()\n  end\nend\n";
        assert_eq!(one(signup, "Oban.insert", TopicRule::ArgReceiver), Some("MailerWorker".into()));
        // One line, arguments to `new`, a multi-line argument list, a match
        // on the result, and the `!` / `_all` spellings of the same needle.
        for src in [
            "%{id: 1} |> EmailWorker.new(queue: :mailers) |> Oban.insert()",
            "EmailWorker.new(%{id: 1}) |> Oban.insert()",
            "args\n|> EmailWorker.new(\n  %{id: 1},\n  max_attempts: 3\n)\n|> Oban.insert()",
            "{:ok, _job} = EmailWorker.new(%{id: \")\"}) |> Oban.insert()",
            "%{id: 1} |> EmailWorker.new() |> Oban.insert!()",
            "%{id: 1} |> EmailWorker.new() |> Oban.insert",
        ] {
            assert_eq!(one(src, "Oban.insert", TopicRule::ArgReceiver), Some("EmailWorker".into()), "{src}");
        }
    }

    #[test]
    fn arg_receiver_pipe_needs_a_new_stage() {
        for src in [
            // The stage is another call.
            "changeset |> Repo.insert() |> Oban.insert()",
            "%{} |> EmailWorker.build() |> Oban.insert()",
            // The `.new(` sits inside another call, not at the stage's end.
            "args |> wrap(EmailWorker.new(args)) |> Oban.insert()",
            // A list of jobs, a variable, no pipe at all.
            "[EmailWorker.new(a)] |> Oban.insert_all()",
            "jobs |> Oban.insert_all()",
            "Oban.insert()",
            "Oban.insert!(EmailWorker.new(%{}))",
            // `.new` with no chain before it.
            "x |> .new() |> Oban.insert()",
        ] {
            assert_eq!(one(src, "Oban.insert", TopicRule::ArgReceiver), None, "{src}");
        }
        // A present argument #0 reads exactly as before, pipe or no pipe.
        assert_eq!(
            one("Oban.insert(EmailWorker.new(%{}))", "Oban.insert", TopicRule::ArgReceiver),
            Some("EmailWorker".into())
        );
        assert_eq!(
            one("x |> Other.new() |> Oban.insert(MyApp.Oban)", "Oban.insert", TopicRule::ArgReceiver),
            None
        );
    }

    // ---- DT.2c: the NATS subject grammar ----------------------------------

    fn folded_as(topic: &str, how: SubjectFold) -> SubjectRead {
        SubjectRead::Folded {
            topic: topic.to_string(),
            how,
        }
    }

    fn lit(topic: &str) -> SubjectRead {
        SubjectRead::Literal((topic.to_string(), TopicForm::Literal))
    }

    /// The one hit `src` holds for `needle` under `rule`.
    fn nats(src: &str, needle: &str, rule: TopicRule) -> TopicHit {
        let mut hits = scan(src, needle, rule);
        assert_eq!(hits.len(), 1, "{src}");
        hits.remove(0)
    }

    #[test]
    fn concat_whole_token_folds() {
        assert_eq!(
            nats_subject("\"shop.shipments.\" + id", ""),
            folded_as("shop.shipments.*", SubjectFold::Concat)
        );
        assert_eq!(nats_subject("id + \".created\"", ""), folded_as("*.created", SubjectFold::Concat));
        assert_eq!(nats_subject("\"a.\" + \"b\"", ""), folded_as("a.b", SubjectFold::Concat));
        assert_eq!(
            nats_subject("\"orders.\" + strconv.Itoa(n)", ""),
            folded_as("orders.*", SubjectFold::Concat)
        );
        let h = nats(
            "conn.Publish(\"shop.shipments.\" + id, []byte(id))",
            ".Publish(",
            TopicRule::NatsSubject,
        );
        assert_eq!(h.topic.as_deref(), Some("shop.shipments.*"));
        assert_eq!(h.fold, Some(SubjectFold::Concat));
        assert!(h.expr.is_none() && !h.dynamic && !h.format);
    }

    #[test]
    fn concat_partial_token_is_dynamic() {
        for arg in [
            "\"orders-\" + region",
            "\"orders.\" + a + b",
            "PREFIX + \".\" + id",
            "\"orders\" + id",
            "\"orders. \" + id",
            "\"orders..\" + id",
            "\"orders.*.\" + id",
        ] {
            assert_eq!(nats_subject(arg, ""), SubjectRead::Dynamic, "{arg}");
        }
        let h = nats("nc.Publish(\"orders-\"+region, nil)", "nc.Publish", TopicRule::NatsSubject);
        assert_eq!(h.topic, None, "never the leading literal `orders-`");
        assert!(h.dynamic && h.expr.is_none() && h.fold.is_none());
    }

    #[test]
    fn concat_inner_token_folds() {
        assert_eq!(
            nats_subject("\"audit.\"+id+\".created\"", ""),
            folded_as("audit.*.created", SubjectFold::Concat)
        );
        assert_eq!(
            nats_subject("\"a.\" + x + \".\" + y", ""),
            folded_as("a.*.*", SubjectFold::Concat)
        );
    }

    #[test]
    fn sprintf_literal_format_folds() {
        assert_eq!(
            nats_subject("fmt.Sprintf(\"shop.orders.%s.placed\", id)", ""),
            folded_as("shop.orders.*.placed", SubjectFold::Sprintf)
        );
        assert_eq!(
            nats_subject("fmt.Sprintf(\"a.%d.%v.%x\", n, v, x)", ""),
            folded_as("a.*.*.*", SubjectFold::Sprintf)
        );
        for arg in [
            "fmt.Sprintf(\"orders-%s\", r)",
            "fmt.Sprintf(\"orders.%q\", r)",
            "fmt.Sprintf(\"orders.%05d\", n)",
            "fmt.Sprintf(\"orders.%[1]s\", r)",
            "fmt.Sprintf(\"%s\", r)",
            "fmt.Sprintf(\"a.%s\", r) + x",
        ] {
            assert_eq!(nats_subject(arg, ""), SubjectRead::Dynamic, "{arg}");
        }
        assert_eq!(subject_from_format("rate.100%%"), Some("rate.100%".to_string()));
        assert_eq!(subject_from_format("shop.refunds.%d").as_deref(), Some("shop.refunds.*"));
        assert_eq!(subject_from_format("plain.subject").as_deref(), Some("plain.subject"));
        let h = nats(
            "nc.Publish(fmt.Sprintf(\"shop.orders.%s.placed\", id), nil)",
            "nc.Publish",
            TopicRule::NatsSubject,
        );
        assert_eq!(h.topic.as_deref(), Some("shop.orders.*.placed"));
        assert_eq!(h.fold, Some(SubjectFold::Sprintf));
    }

    #[test]
    fn sprintf_identifier_format_is_a_format_expression() {
        assert_eq!(
            nats_subject("fmt.Sprintf(RefundSubject, n)", ""),
            SubjectRead::Expr {
                chain: "RefundSubject".to_string(),
                format: true
            }
        );
        let h = nats(
            "p.conn.Publish(fmt.Sprintf(p.template, o.ID), payload)",
            ".Publish(",
            TopicRule::NatsSubject,
        );
        assert_eq!(h.expr.as_deref(), Some("p.template"));
        assert!(h.format && h.topic.is_none() && !h.dynamic);
        // A plain identifier is the LA.4 expression it always was.
        let h = nats("nc.Publish(SubjectOrders, d)", "nc.Publish", TopicRule::NatsSubject);
        assert_eq!(h.expr.as_deref(), Some("SubjectOrders"));
        assert!(!h.format);
    }

    #[test]
    fn same_file_single_return_func_folds() {
        let one_line = "package p\n\n// shipment names the subject.\nfunc shipment(id string) string { return \"shop.shipments.\" + id }\n\nfunc P(conn *nats.Conn, id string) error {\n\treturn conn.Publish(shipment(id), []byte(id))\n}\n";
        let h = nats(one_line, ".Publish(", TopicRule::NatsSubject);
        assert_eq!(h.topic.as_deref(), Some("shop.shipments.*"));
        assert_eq!(h.fold, Some(SubjectFold::Func));
        let multi = "package p\n\n// notifSubject is one user's subject.\nfunc notifSubject(user string) string {\n\t// trimmed, the user id is the last token\n\treturn \"notif.\" +\n\t\tstrings.TrimSpace(user) // the token\n}\n\nfunc N(nc *nats.Conn, u string) { nc.Publish(notifSubject(u), nil) }\n";
        let h = nats(multi, "nc.Publish", TopicRule::NatsSubject);
        assert_eq!(h.topic.as_deref(), Some("notif.*"));
        assert_eq!(h.fold, Some(SubjectFold::Func));
        // A literal or a Sprintf body folds the same way, at depth 1.
        let src = "func subj() string {\n\treturn \"orders.created\"\n}\nfunc fs(id string) string { return fmt.Sprintf(\"orders.%s\", id) }\n";
        assert_eq!(nats_subject("subj()", src), folded_as("orders.created", SubjectFold::Func));
        assert_eq!(nats_subject("fs(id)", src), folded_as("orders.*", SubjectFold::Func));
    }

    #[test]
    fn multi_statement_func_is_dynamic() {
        let room = "func roomSubject(id string) (string, error) {\n\treturn \"rooms.\" + id, nil\n}\nfunc guarded(id string) string {\n\tif id == \"\" {\n\t\treturn \"rooms.lobby\"\n\t}\n\treturn \"rooms.\" + id\n}\nfunc local(id string) string {\n\ts := \"rooms.\" + id\n\treturn s\n}\nfunc semi(id string) string { x := id; return \"rooms.\" + x }\nfunc (p *P) method(id string) string { return \"rooms.\" + id }\nfunc nested(id string) string { return shipment(id) }\nfunc shipment(id string) string { return \"shop.shipments.\" + id }\nfunc param(id string) string { return id }\n";
        for arg in [
            "roomSubject(id)",
            "guarded(id)",
            "local(id)",
            "semi(id)",
            "method(id)",
            "nested(id)",
            "param(id)",
            "elsewhere(id)",
            "p.subject(id)",
        ] {
            assert_eq!(nats_subject(arg, room), SubjectRead::Dynamic, "{arg}");
        }
        // A zero-argument call no function explains stays the LA.4 chain.
        assert_eq!(
            nats_subject("subject()", room),
            SubjectRead::Expr {
                chain: "subject()".to_string(),
                format: false
            }
        );
    }

    #[test]
    fn js_template_python_fstring_ruby_interp_fold() {
        let cases: &[(&str, &str)] = &[
            ("`orders.${id}.placed`", "orders.*.placed"),
            ("f\"orders.{order_id}.placed\"", "orders.*.placed"),
            ("rf'orders.{order_id}'", "orders.*"),
            ("\"orders.#{id}.placed\"", "orders.*.placed"),
            ("\"orders.$id.placed\"", "orders.*.placed"),
            ("\"orders.${order.id}\"", "orders.*"),
            ("s\"orders.$id\"", "orders.*"),
            ("$\"orders.{id}.placed\"", "orders.*.placed"),
        ];
        for (arg, want) in cases {
            assert_eq!(nats_subject(arg, ""), folded_as(want, SubjectFold::Template), "{arg}");
        }
        // The measured HEAD shapes, through the needles that read them.
        let js = nats("nc.publish(`orders.${id}.placed`)", "nc.publish", TopicRule::NatsSubject);
        assert_eq!(js.topic.as_deref(), Some("orders.*.placed"));
        let js = nats("nc.publish('orders.' + id)", "nc.publish", TopicRule::NatsSubject);
        assert_eq!(js.topic.as_deref(), Some("orders.*"));
        let py = nats(
            "await nc.publish(f\"orders.{order_id}.placed\", b\"\")",
            "nc.publish",
            TopicRule::NatsSubject,
        );
        assert_eq!(py.topic.as_deref(), Some("orders.*.placed"));
        // No interpolation reads as the literal; a NATS system subject's
        // `$JS` is text; a partial interpolated token is dynamic.
        assert_eq!(nats_subject("\"orders.created\"", ""), lit("orders.created"));
        assert_eq!(nats_subject("'$JS.API.STREAM.INFO'", ""), lit("$JS.API.STREAM.INFO"));
        assert_eq!(nats_subject("`orders-${region}`", ""), SubjectRead::Dynamic);
        assert_eq!(nats_subject("`orders.${id`", ""), SubjectRead::Dynamic);
        // A named argument reads its value.
        assert_eq!(nats_subject("subject=\"orders\"", ""), lit("orders"));
        assert_eq!(
            nats_subject("subject: \"orders.\" + id", ""),
            folded_as("orders.*", SubjectFold::Concat)
        );
    }

    #[test]
    fn consumer_filter_never_folds() {
        assert_eq!(nats_filter("\"orders.*\""), lit("orders.*"));
        assert_eq!(nats_filter("'orders.>'"), lit("orders.>"));
        assert_eq!(
            nats_filter("subj"),
            SubjectRead::Expr {
                chain: "subj".to_string(),
                format: false
            }
        );
        for arg in [
            "\"orders.\"+r",
            "`orders.${id}`",
            "f\"orders.{x}\"",
            "fmt.Sprintf(\"orders.%s\", r)",
            "subjectFor(r)",
        ] {
            assert_eq!(nats_filter(arg), SubjectRead::Dynamic, "{arg}");
        }
        let h = nats("nc.Subscribe(\"orders.\"+r, h)", "nc.Subscribe", TopicRule::NatsFilter);
        assert_eq!(h.topic, None, "never the leading literal `orders.`");
        assert!(h.dynamic);
    }

    #[test]
    fn nats_hits_keep_the_head_slot_rules() {
        // Behind a context the subject is argument #1; a prefix-matched
        // `nc.PublishMsg(` holds no region and reads nothing, not dynamic.
        let h = nats("js.Publish(ctx, \"orders.\" + id, d)", ".Publish(", TopicRule::NatsSubject);
        assert_eq!(h.topic.as_deref(), Some("orders.*"));
        let h = nats("nc.PublishMsg(&nats.Msg{Subject: \"orders\"})", "nc.Publish", TopicRule::NatsSubject);
        assert!(h.topic.is_none() && h.expr.is_none() && !h.dynamic);
        let h = nats("nc.Publish()", "nc.Publish", TopicRule::NatsSubject);
        assert!(h.topic.is_none() && h.expr.is_none() && !h.dynamic);
        // A multibyte subject never panics.
        let h = nats("nc.Publish(\"ordérs.\" + id, nil)", "nc.Publish", TopicRule::NatsSubject);
        assert_eq!(h.topic.as_deref(), Some("ordérs.*"));
        let h = nats("nc.Publish(`é${x}`, nil)", "nc.Publish", TopicRule::NatsSubject);
        assert!(h.dynamic);
    }
}
