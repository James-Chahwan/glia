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
}

/// Bound on occurrences examined per needle per file — generated files can
/// contain thousands of identical calls and every hit costs a region walk.
pub const MAX_HITS_PER_NEEDLE: usize = 32;

/// Bound on the argument region walked for one occurrence.
pub const MAX_REGION: usize = 2048;

/// Longest topic accepted. Anything longer is a payload, not an identifier.
pub const MAX_TOPIC_LEN: usize = 128;

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
        let topic = match rule {
            TopicRule::NoIdentity => None,
            _ => topic_at(source, offset.saturating_add(needle.len()), needle, rule),
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

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

fn topic_at(source: &str, after: usize, needle: &str, rule: TopicRule) -> Option<String> {
    let region = arg_region(source, after, needle)?;
    match rule {
        TopicRule::ArgLiteral => arg_literal(region, 0),
        TopicRule::ArgIndex(n) => arg_literal(region, n),
        TopicRule::Keyed(keys) => keyed_literal(region, keys),
        TopicRule::KeyedOrArg(keys) => {
            keyed_literal(region, keys).or_else(|| arg_literal(region, 0))
        }
        TopicRule::NoIdentity => None,
    }
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

    #[test]
    fn clip_never_splits_a_char() {
        assert_eq!(clip("日本語", 4), "日");
        assert_eq!(clip("abc", 99), "abc");
    }
}
