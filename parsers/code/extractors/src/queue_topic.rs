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
/// class and not as a `public` whose next word is `class`.
const DEF_KEYWORDS: &[&str] = &[
    "def ",
    "fn ",
    "func ",
    "function ",
    "class ",
    "void ",
    "public ",
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
    let sites = source
        .match_indices(needle)
        .filter(|&(offset, _)| bare_word_ok(source, offset, needle, rule) && guard.admits(offset))
        .take(MAX_HITS_PER_NEEDLE);
    for (offset, _) in sites {
        let after = offset.saturating_add(needle.len());
        let read = match rule {
            TopicRule::NoIdentity => None,
            // A2.5: identity rules read the code AROUND the call, not its
            // arguments, so they need the needle's own offset. A symbol is
            // never a URL/ARN, so its form is always `Literal`.
            TopicRule::Receiver => verbatim(receiver_before(source, offset)),
            TopicRule::ArgReceiver => verbatim(arg_receiver(source, after, needle)),
            TopicRule::DeclaredSymbol => verbatim(declared_symbol(source, after)),
            TopicRule::EnclosingSymbol => verbatim(enclosing_symbol(source, offset)),
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
        // `NoIdentity` and the A2.5 identity rules never read an argument
        // region; `scan` dispatches them before it gets here.
        _ => None,
    }
}

/// LA.4: the identifier expression in the slot [`topic_at`] read no literal
/// from — the same region, the same argument, the same key, tried in the same
/// order. `NoIdentity` and the A2.5 identity rules never carry one.
fn expr_at(source: &str, after: usize, needle: &str, rule: TopicRule) -> Option<String> {
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
        _ => None,
    }
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
pub(crate) fn arg_region<'a>(source: &'a str, after: usize, needle: &str) -> Option<&'a str> {
    let rest = source.get(after..)?;
    if needle.ends_with(['(', '{', '[']) {
        return region_body(rest);
    }
    let trimmed = rest.trim_start();
    if !matches!(trimmed.as_bytes().first(), Some(b'(' | b'{' | b'[')) {
        return None;
    }
    region_body(trimmed.get(1..)?)
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
    // ASCII-lowercasing is byte-length preserving, so indices map 1:1 back onto
    // `region` and we can read the ORIGINAL bytes at an index found in `lower`.
    let lower = region.to_ascii_lowercase();
    debug_assert_eq!(lower.len(), region.len());
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
            // optional closing quote of a quoted key: `'topic' => 'x'`
            if matches!(b.get(j), Some(b'\'' | b'"' | b'`')) {
                j += 1;
            }
            j = skip_ws(b, j);
            let builder = b.get(j) == Some(&b'(') && start > 0 && b.get(start - 1) == Some(&b'.');
            let Some(sep) = separator_len(b, j).or(builder.then_some(1)) else {
                continue;
            };
            j = skip_ws(b, j + sep);
            // CL.2: `QueueUrl: aws.String("..")` reads the literal inside.
            let j = step_into_helper(region, j).unwrap_or(j);
            if let Some(topic) = read_literal(region, j) {
                return Some(topic);
            }
        }
    }
    None
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
fn bare_word_ok(source: &str, offset: usize, needle: &str, rule: TopicRule) -> bool {
    let bare = matches!(rule, TopicRule::Receiver)
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

/// [`TopicRule::Receiver`] — the chain immediately before the needle.
fn receiver_before(source: &str, offset: usize) -> Option<String> {
    ident_before(source.get(..offset)?)
}

/// [`TopicRule::ArgReceiver`] — the callee of positional argument #0.
fn arg_receiver(source: &str, after: usize, needle: &str) -> Option<String> {
    let region = arg_region(source, after, needle)?;
    let arg = *split_args(region).first()?;
    let call = arg.find('(')?;
    last_symbol(arg.get(..call)?)
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
            return Some(name);
        }
    }
    None
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
}
