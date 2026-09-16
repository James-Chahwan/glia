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

/// One occurrence of a needle in a source file.
pub struct TopicHit {
    /// The topic this occurrence names, if the rule could read one.
    pub topic: Option<String>,
    /// Byte index of the needle in the source (stable, for ordering/debug).
    pub offset: usize,
}

/// How a topic is spelled at a given needle.
#[derive(Clone, Copy, Debug)]
pub enum TopicRule {
    /// First quoted literal inside positional argument #0.
    ArgLiteral,
    /// First quoted literal inside positional argument N (AMQP
    /// `basic_publish(exchange, routing_key)` and friends).
    ArgIndex(usize),
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
pub fn scan(source: &str, needle: &str, rule: TopicRule) -> Vec<TopicHit> {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    for (offset, _) in source.match_indices(needle).take(MAX_HITS_PER_NEEDLE) {
        let after = offset.saturating_add(needle.len());
        let topic = match rule {
            TopicRule::NoIdentity => None,
            // A2.5: identity rules read the code AROUND the call, not its
            // arguments, so they need the needle's own offset.
            TopicRule::Receiver => receiver_before(source, offset),
            TopicRule::ArgReceiver => arg_receiver(source, after, needle),
            TopicRule::DeclaredSymbol => declared_symbol(source, after),
            TopicRule::EnclosingSymbol => enclosing_symbol(source, offset),
            _ => topic_at(source, after, needle, rule),
        };
        hits.push(TopicHit { topic, offset });
    }
    hits
}

/// Trim quoting/punctuation noise off a raw literal and reject non-identifiers.
pub fn normalise_topic(raw: &str) -> Option<String> {
    let t = raw
        .trim()
        .trim_matches(|c: char| matches!(c, ',' | ')' | ']' | '}' | '\'' | '"' | '`'))
        .trim();
    if t.is_empty() || t.len() > MAX_TOPIC_LEN || t.contains('\n') || t.contains('\r') {
        return None;
    }
    Some(t.to_string())
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
/// `repo_graph_code_domain` helper should replace both.
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

fn topic_at(source: &str, after: usize, needle: &str, rule: TopicRule) -> Option<String> {
    let region = arg_region(source, after, needle);
    match rule {
        TopicRule::ArgLiteral => arg_literal(region?, 0),
        TopicRule::ArgIndex(n) => arg_literal(region?, n),
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
fn arg_region<'a>(source: &'a str, after: usize, needle: &str) -> Option<&'a str> {
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
fn region_body(s: &str) -> Option<&str> {
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
fn split_args(region: &str) -> Vec<&str> {
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

/// First quoted literal inside positional argument `n`.
fn arg_literal(region: &str, n: usize) -> Option<String> {
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
fn keyed_literal(region: &str, keys: &[&str]) -> Option<String> {
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
            let Some(sep) = separator_len(b, j) else {
                continue;
            };
            j = skip_ws(b, j + sep);
            if let Some(topic) = read_literal(region, j) {
                return Some(topic);
            }
        }
    }
    None
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
fn read_literal(s: &str, mut j: usize) -> Option<String> {
    let b = s.as_bytes();
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
                    return normalise_topic(s.get(start..j)?);
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
                normalise_topic(s.get(start..j)?)
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
}
