//! The Tier lattice, Value / ValueRef, the per-evaluation ValueTable and Lookup.
//!
//! A value inside an evaluation is a `Handle`: a u32 index into one `ValueTable`
//! that interns node ids, strings and small ints, so a relation row is a slice of
//! handles and an equality join compares integers. Handles are never persisted and
//! never order output: handle order is interning order, which follows the fact
//! source's order and so differs between checkouts (NodeIds hash a path-derived
//! RepoId). The one output order is `ValueTable::cmp_rendered`: type rank, then
//! the value, a node by its rendered key.
//!
//! `Lookup` is the crate's only hash map, and it offers lookups and inserts with no
//! way to iterate, so map order cannot reach an output by construction. DA.19's
//! source guard pins that no other module names a std map and that this file never
//! walks Lookup's inner map.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

use glia_core::NodeId;

/// The evidence tier, ordered by weakness: FACT (read at a site) < DERIVED (paired
/// by a resolver or pass) < HEURISTIC (a guess). A conjunction is as weak as its
/// weakest part ([`Tier::weakest`], max); of alternative derivations the best one
/// counts ([`Tier::best`], min). The one tier type every crate reads (FD.7a), from
/// `glia_activation::profile`; re-exported here so rules code keeps its path.
pub use glia_activation::profile::Tier;

/// An owned value: a node, a string or an int. `Str("3")` and `Int(3)` are distinct.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Value {
    Node(NodeId),
    Str(String),
    Int(i64),
}

/// A borrowed [`Value`], the form values are interned, found and passed to builtins in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueRef<'a> {
    Node(NodeId),
    Str(&'a str),
    Int(i64),
}

impl Value {
    pub fn as_ref(&self) -> ValueRef<'_> {
        match self {
            Value::Node(n) => ValueRef::Node(*n),
            Value::Str(s) => ValueRef::Str(s),
            Value::Int(i) => ValueRef::Int(*i),
        }
    }
}

impl ValueRef<'_> {
    pub fn to_owned(self) -> Value {
        match self {
            ValueRef::Node(n) => Value::Node(n),
            ValueRef::Str(s) => Value::Str(s.to_string()),
            ValueRef::Int(i) => Value::Int(i),
        }
    }
}

/// A value's index into the [`ValueTable`] of the evaluation that interned it.
pub(crate) type Handle = u32;

/// The empty slot of a [`SlotIndex`]. No value gets this handle: interning stops one
/// short of it, a bound no evaluation reaches (the row budget caps derived rows at
/// millions and a fact source holding four billion distinct values does not fit in
/// memory).
const EMPTY_SLOT: Handle = Handle::MAX;

/// The per-evaluation value interner. Each distinct value is stored once, so a
/// qname attribute column holds handles, not a second copy of every qname.
///
/// The index ([`SlotIndex`]) holds each handle once, at a slot its value's 64-bit
/// digest picks, probing on; a lookup compares the values it meets, so a digest
/// collision costs a longer probe, never a wrong handle. The digest is a fixed mix
/// ([`mix64`], no random seed), so the slots are the same on every run (GA.13: 4 bytes
/// a slot, where a digest-keyed map with collision chains held about 30 a value).
#[derive(Clone, Debug)]
pub(crate) struct ValueTable {
    data: Vec<Value>,
    index: SlotIndex,
    /// The digest every lookup and every slot uses ([`digest`]; tests force
    /// collisions with another).
    digest: fn(ValueRef<'_>) -> u64,
    /// Per handle, filled on first use: the cached rendered key of a node or int; a
    /// string is its own key.
    rendered: Vec<Option<Box<str>>>,
}

impl Default for ValueTable {
    fn default() -> Self {
        ValueTable {
            data: Vec::new(),
            index: SlotIndex::default(),
            digest,
            rendered: Vec::new(),
        }
    }
}

/// The type rank output order uses: Int < Str < Node.
fn type_rank(v: ValueRef<'_>) -> u8 {
    match v {
        ValueRef::Int(_) => 0,
        ValueRef::Str(_) => 1,
        ValueRef::Node(_) => 2,
    }
}

/// splitmix64's finaliser: a fixed, well-spread 64-bit mix (no seed, so a digest is
/// the same on every run). A [`SlotIndex`] picks its slot from the low bits.
pub(crate) fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The interning digest: a type tag, then the payload, so `Str("3")` and `Int(3)`
/// start apart before their bytes are read (equal digests only chain, never merge).
fn digest(v: ValueRef<'_>) -> u64 {
    match v {
        ValueRef::Node(n) => mix64(n.0 ^ 0x4E4F_4445),
        ValueRef::Int(i) => mix64(i.cast_unsigned() ^ 0x494E_5400_0000_0000),
        ValueRef::Str(s) => {
            let bytes = s.as_bytes();
            let mut h = mix64(0x5354_5200 ^ bytes.len() as u64);
            for chunk in bytes.chunks(8) {
                let mut word = [0u8; 8];
                word[..chunk.len()].copy_from_slice(chunk);
                h = mix64(h ^ u64::from_le_bytes(word));
            }
            h
        }
    }
}

/// An open-addressing index of `u32` entries whose keys live with its caller (GA.13):
/// a power-of-two slot table, linear probing from the slot a key's [`mix64`]-spread
/// digest picks, at most three quarters full. It stores no key and no digest; a lookup
/// asks the caller whether an entry is the one sought, and a growth asks it for each
/// entry's digest again. No entry is [`EMPTY_SLOT`]. Never iterated into an output.
#[derive(Clone, Debug, Default)]
pub(crate) struct SlotIndex {
    slots: Vec<u32>,
    len: usize,
}

impl SlotIndex {
    /// The entry under digest `d` that `is` accepts.
    pub(crate) fn find(&self, d: u64, mut is: impl FnMut(u32) -> bool) -> Option<u32> {
        let mask = self.slots.len().checked_sub(1)?;
        let mut i = (d as usize) & mask;
        loop {
            let e = self.slots[i];
            if e == EMPTY_SLOT {
                return None;
            }
            if is(e) {
                return Some(e);
            }
            i = (i + 1) & mask;
        }
    }

    /// Add entry `e` (never [`EMPTY_SLOT`]) under digest `d`, the caller having found
    /// it absent; `digest_of` gives each held entry's digest when the slots grow.
    pub(crate) fn insert(&mut self, d: u64, e: u32, digest_of: impl Fn(u32) -> u64) {
        debug_assert_ne!(e, EMPTY_SLOT);
        if (self.len + 1) * 4 > self.slots.len() * 3 {
            let old = std::mem::take(&mut self.slots);
            self.slots = vec![EMPTY_SLOT; (old.len() * 2).max(16)];
            for held in old.into_iter().filter(|&h| h != EMPTY_SLOT) {
                self.place(digest_of(held), held);
            }
        }
        self.place(d, e);
        self.len += 1;
    }

    fn place(&mut self, d: u64, e: u32) {
        let mask = self.slots.len() - 1;
        let mut i = (d as usize) & mask;
        while self.slots[i] != EMPTY_SLOT {
            i = (i + 1) & mask;
        }
        self.slots[i] = e;
    }

    /// Heap bytes held (the slot table's capacity).
    pub(crate) fn heap_bytes(&self) -> usize {
        self.slots.capacity() * std::mem::size_of::<u32>()
    }
}

impl ValueTable {
    pub(crate) fn new() -> Self {
        ValueTable::default()
    }

    /// The handle of `v`, interning it on first sight. Idempotent: the same value
    /// always gets the same handle within one table.
    pub(crate) fn intern(&mut self, v: ValueRef<'_>) -> Handle {
        let d = (self.digest)(v);
        if let Some(h) = self.find_at(d, v) {
            return h;
        }
        debug_assert!(self.data.len() < EMPTY_SLOT as usize, "value table full");
        let h = self.data.len() as Handle;
        self.data.push(v.to_owned());
        let (data, digest) = (&self.data, self.digest);
        self.index
            .insert(d, h, |e| digest(data[e as usize].as_ref()));
        h
    }

    /// The handle of `v` if it was interned, without interning it.
    pub(crate) fn find(&self, v: ValueRef<'_>) -> Option<Handle> {
        self.find_at((self.digest)(v), v)
    }

    /// The value behind `h`. `h` must come from this table.
    pub(crate) fn get(&self, h: Handle) -> ValueRef<'_> {
        self.data[h as usize].as_ref()
    }

    /// The number of distinct values interned.
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    /// The rendered key of `h`: a node's `render(id)` (computed once per handle and
    /// cached), an int's decimal, a string itself.
    pub(crate) fn rendered(&mut self, h: Handle, render: &dyn Fn(NodeId) -> String) -> &str {
        let i = h as usize;
        if self.rendered.len() <= i {
            self.rendered
                .resize_with(self.data.len().max(i + 1), || None);
        }
        let slot = &mut self.rendered[i];
        match &self.data[i] {
            Value::Str(s) => s.as_str(),
            Value::Node(n) => slot.get_or_insert_with(|| render(*n).into_boxed_str()),
            Value::Int(x) => slot.get_or_insert_with(|| x.to_string().into_boxed_str()),
        }
    }

    /// The output order, checkout-stable: type rank Int < Str < Node, then an int
    /// numerically, a string by bytes, a node by its rendered key. Two nodes a domain
    /// renders identically fall back to NodeId order, the one tie that is not
    /// checkout-stable and is reached only through such a domain.
    pub(crate) fn cmp_rendered(
        &mut self,
        a: Handle,
        b: Handle,
        render: &dyn Fn(NodeId) -> String,
    ) -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        let (va, vb) = (self.get(a), self.get(b));
        match (va, vb) {
            (ValueRef::Int(x), ValueRef::Int(y)) => x.cmp(&y),
            // str's Ord is bytewise.
            (ValueRef::Str(x), ValueRef::Str(y)) => x.cmp(y),
            (ValueRef::Node(x), ValueRef::Node(y)) => {
                self.rendered(a, render);
                self.rendered(b, render);
                let (ka, kb) = (&self.rendered[a as usize], &self.rendered[b as usize]);
                ka.cmp(kb).then(x.0.cmp(&y.0))
            }
            _ => type_rank(va).cmp(&type_rank(vb)),
        }
    }

    /// The `<` family's comparison: ints numerically, strings by bytes. `None` for a
    /// node (the caller orders nodes with [`ValueTable::cmp_rendered`]) and for two
    /// values of different types (the comparison is false).
    pub(crate) fn cmp_values(&self, a: Handle, b: Handle) -> Option<Ordering> {
        match (self.get(a), self.get(b)) {
            (ValueRef::Int(x), ValueRef::Int(y)) => Some(x.cmp(&y)),
            (ValueRef::Str(x), ValueRef::Str(y)) => Some(x.cmp(y)),
            _ => None,
        }
    }

    /// Heap bytes held, by capacity: the value column, every interned string, the
    /// render cache and its keys, and the slot index. Read-only accounting for
    /// `[rules] mem` (GA.13).
    pub(crate) fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        let strings: usize = self
            .data
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.capacity(),
                Value::Node(_) | Value::Int(_) => 0,
            })
            .sum();
        let rendered: usize = self.rendered.iter().flatten().map(|r| r.len()).sum();
        self.data.capacity() * size_of::<Value>()
            + strings
            + self.rendered.capacity() * size_of::<Option<Box<str>>>()
            + rendered
            + self.index.heap_bytes()
    }

    /// The handle of `v` under digest `d` (this table's digest of `v`).
    fn find_at(&self, d: u64, v: ValueRef<'_>) -> Option<Handle> {
        self.index.find(d, |h| self.data[h as usize].as_ref() == v)
    }
}

/// A hash map with lookups only: get, insert, get_or_insert_with, contains_key,
/// len. It has no iter / keys / values / drain and no IntoIterator, so
/// its order cannot reach an output. The inner HashMap is a private field: no module
/// but this one can reach it. Debug prints the length only.
#[derive(Clone)]
pub(crate) struct Lookup<K, V>(HashMap<K, V>);

impl<K, V> Default for Lookup<K, V> {
    fn default() -> Self {
        Lookup(HashMap::new())
    }
}

impl<K, V> fmt::Debug for Lookup<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lookup")
            .field("len", &self.0.len())
            .finish()
    }
}

impl<K: Hash + Eq, V> Lookup<K, V> {
    pub(crate) fn new() -> Self {
        Lookup::default()
    }

    pub(crate) fn get<Q>(&self, k: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.get(k)
    }

    /// Inserts `v` at `k`, returning the value it replaced.
    pub(crate) fn insert(&mut self, k: K, v: V) -> Option<V> {
        self.0.insert(k, v)
    }

    /// The value at `k`, inserting `f()` first when there is none.
    pub(crate) fn get_or_insert_with(&mut self, k: K, f: impl FnOnce() -> V) -> &mut V {
        self.0.entry(k).or_insert_with(f)
    }

    pub(crate) fn contains_key<Q>(&self, k: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.contains_key(k)
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn tier_is_ordered_by_weakness() {
        assert!(Tier::Fact < Tier::Derived && Tier::Derived < Tier::Heuristic);
        assert_eq!(Tier::default(), Tier::Fact);
        assert_eq!(Tier::Fact.weakest(Tier::Heuristic), Tier::Heuristic);
        assert_eq!(Tier::Heuristic.weakest(Tier::Fact), Tier::Heuristic);
        assert_eq!(Tier::Derived.best(Tier::Fact), Tier::Fact);
        assert_eq!(Tier::Fact.best(Tier::Derived), Tier::Fact);
        let names: Vec<_> = Tier::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(names, ["fact", "derived", "heuristic"]);
        let upper: Vec<_> = Tier::ALL.iter().map(|t| t.upper()).collect();
        assert_eq!(upper, ["FACT", "DERIVED", "HEURISTIC"]);
        assert_eq!(Tier::parse("DERIVED"), Some(Tier::Derived));
        for t in Tier::ALL {
            assert_eq!(Tier::parse(t.as_str()), Some(t));
            assert_eq!(Tier::parse(t.upper()), Some(t));
        }
        assert_eq!(Tier::parse("strong"), None);
        assert_eq!(Tier::parse(""), None);
        for l in 0..=2u8 {
            let t = Tier::from_level(l).expect("levels 0..=2 are tiers");
            assert_eq!(t.level(), l);
            assert_eq!(Tier::ALL[usize::from(l)], t);
        }
        assert_eq!(Tier::from_level(3), None);
    }

    #[test]
    fn intern_is_idempotent_and_typed() {
        let mut t = ValueTable::new();
        let s = t.intern(ValueRef::Str("3"));
        assert_eq!(t.intern(ValueRef::Str("3")), s);
        let i = t.intern(ValueRef::Int(3));
        let n = t.intern(ValueRef::Node(NodeId(3)));
        assert!(s != i && i != n && s != n);
        assert_eq!(t.intern(ValueRef::Int(3)), i);
        assert_eq!(t.intern(Value::Node(NodeId(3)).as_ref()), n);
        assert_eq!(t.find(ValueRef::Str("3")), Some(s));
        assert_eq!(t.find(ValueRef::Int(3)), Some(i));
        assert_eq!(t.find(ValueRef::Node(NodeId(3))), Some(n));
        assert_eq!(t.find(ValueRef::Str("4")), None);
        assert_eq!(t.find(ValueRef::Int(4)), None);
        assert_eq!(t.find(ValueRef::Node(NodeId(4))), None);
        assert_eq!(t.len(), 3);
        assert_eq!(t.get(s), ValueRef::Str("3"));
        assert_eq!(t.get(i).to_owned(), Value::Int(3));
        assert_eq!(t.get(n), ValueRef::Node(NodeId(3)));
    }

    #[test]
    fn collisions_chain() {
        let mut t = ValueTable::new();
        let words: Vec<String> = (0..10_000).map(|i| format!("w{i}")).collect();
        let handles: Vec<Handle> = words.iter().map(|w| t.intern(ValueRef::Str(w))).collect();
        let mut distinct = handles.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 10_000);
        assert_eq!(t.len(), 10_000);
        for (w, h) in words.iter().zip(&handles) {
            assert_eq!(t.find(ValueRef::Str(w)), Some(*h));
            assert_eq!(t.get(*h), ValueRef::Str(w.as_str()));
        }

        // Forced collisions: 1,000 values over 3 digests, probes ~333 long.
        let mut c = ValueTable {
            digest: |v| digest(v) % 3,
            ..ValueTable::new()
        };
        let vals: Vec<Value> = (0..1_000)
            .map(|i| match i % 3 {
                0 => Value::Str(i.to_string()),
                1 => Value::Int(i),
                _ => Value::Node(NodeId(i as u64)),
            })
            .collect();
        let hs: Vec<Handle> = vals.iter().map(|v| c.intern(v.as_ref())).collect();
        assert_eq!(hs, (0..1_000).collect::<Vec<Handle>>());
        for (i, v) in vals.iter().enumerate() {
            assert_eq!(c.find(v.as_ref()), Some(hs[i]));
            assert_eq!(c.intern(v.as_ref()), hs[i]);
        }
        assert_eq!(c.find(ValueRef::Str("1001")), None);
        assert_eq!(c.len(), 1_000);
    }

    #[test]
    fn cmp_rendered_orders_by_type_then_value() {
        let (a, b) = (NodeId(900), NodeId(100));
        let (twin_lo, twin_hi) = (NodeId(5), NodeId(7));
        let calls = Cell::new(0u32);
        let render = |n: NodeId| {
            calls.set(calls.get() + 1);
            match n.0 {
                900 => "a::x".to_string(),
                100 => "b::y".to_string(),
                _ => "c::twin".to_string(),
            }
        };
        let mut t = ValueTable::new();
        // Interned in an order unrelated to the expected output order.
        let hb = t.intern(ValueRef::Node(b));
        let h9 = t.intern(ValueRef::Str("9"));
        let hi = t.intern(ValueRef::Node(twin_hi));
        let h10 = t.intern(ValueRef::Int(10));
        let ha = t.intern(ValueRef::Node(a));
        let hs10 = t.intern(ValueRef::Str("10"));
        let hlo = t.intern(ValueRef::Node(twin_lo));
        let h2 = t.intern(ValueRef::Int(2));
        let want = [h2, h10, hs10, h9, ha, hb, hlo, hi];
        for w in want.windows(2) {
            assert_eq!(t.cmp_rendered(w[0], w[1], &render), Ordering::Less, "{w:?}");
            assert_eq!(
                t.cmp_rendered(w[1], w[0], &render),
                Ordering::Greater,
                "{w:?}"
            );
        }
        assert_eq!(t.cmp_rendered(ha, ha, &render), Ordering::Equal);
        let mut got = vec![hb, h9, hi, h10, ha, hs10, hlo, h2];
        got.sort_by(|x, y| t.cmp_rendered(*x, *y, &render));
        assert_eq!(got, want);
        // b's NodeId is below a's, yet a renders first.
        assert!(b.0 < a.0);
        // Each node handle rendered once, then read from the cache.
        assert_eq!(calls.get(), 4);
        assert_eq!(t.rendered(ha, &render), "a::x");
        assert_eq!(t.rendered(h10, &render), "10");
        assert_eq!(t.rendered(h9, &render), "9");
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn cmp_values() {
        let mut t = ValueTable::new();
        let (i2, i10, ineg) = (
            t.intern(ValueRef::Int(2)),
            t.intern(ValueRef::Int(10)),
            t.intern(ValueRef::Int(-1)),
        );
        let (s10, s9, sb) = (
            t.intern(ValueRef::Str("10")),
            t.intern(ValueRef::Str("9")),
            t.intern(ValueRef::Str("b")),
        );
        let (n1, n2) = (
            t.intern(ValueRef::Node(NodeId(1))),
            t.intern(ValueRef::Node(NodeId(2))),
        );
        assert_eq!(t.cmp_values(i2, i10), Some(Ordering::Less));
        assert_eq!(t.cmp_values(ineg, i2), Some(Ordering::Less));
        assert_eq!(t.cmp_values(i10, i10), Some(Ordering::Equal));
        assert_eq!(t.cmp_values(s10, s9), Some(Ordering::Less));
        assert_eq!(t.cmp_values(sb, s9), Some(Ordering::Greater));
        assert_eq!(t.cmp_values(n1, n2), None);
        assert_eq!(t.cmp_values(n1, n1), None);
        assert_eq!(t.cmp_values(i10, s10), None);
        assert_eq!(t.cmp_values(s9, n1), None);
    }

    #[test]
    fn slot_index_and_digests() {
        // Keys live with the caller: here entry e's key is e * 7.
        let key = |e: u32| u64::from(e) * 7;
        let mut ix = SlotIndex::default();
        assert_eq!(ix.find(mix64(0), |_| true), None, "empty");
        for e in 0..1_000u32 {
            assert_eq!(ix.find(mix64(key(e)), |h| key(h) == key(e)), None);
            ix.insert(mix64(key(e)), e, |h| mix64(key(h)));
        }
        for e in 0..1_000u32 {
            assert_eq!(ix.find(mix64(key(e)), |h| key(h) == key(e)), Some(e));
        }
        assert_eq!(ix.find(mix64(key(1_000)), |h| key(h) == key(1_000)), None);
        // At most three quarters full: 1,000 entries take 2,048 slots.
        assert_eq!(ix.heap_bytes(), 2_048 * 4);
        // The digest is fixed (the same on every run) and typed.
        assert_eq!(mix64(0), 0xE220_A839_7B1D_CDAF);
        let (n, i, st) = (
            digest(ValueRef::Node(NodeId(3))),
            digest(ValueRef::Int(3)),
            digest(ValueRef::Str("3")),
        );
        assert!(n != i && i != st && n != st);
        assert_ne!(
            digest(ValueRef::Str("abcdefgh")),
            digest(ValueRef::Str("abcdefgh\0"))
        );
    }

    #[test]
    fn lookup_is_a_map() {
        let mut m: Lookup<u32, u32> = Lookup::new();
        assert_eq!(m.len(), 0);
        for k in 0..1_000u32 {
            assert_eq!(m.insert(k, k * 2), None);
        }
        assert_eq!(m.len(), 1_000);
        for k in 0..1_000u32 {
            assert_eq!(m.get(&k), Some(&(k * 2)));
            assert!(m.contains_key(&k));
        }
        assert_eq!(m.get(&1_000), None);
        assert!(!m.contains_key(&1_000));
        for k in (0..1_000u32).step_by(2) {
            *m.get_or_insert_with(k, || 0) += 1;
        }
        assert_eq!(m.get(&4), Some(&9));
        assert_eq!(m.get(&5), Some(&10));
        assert_eq!(m.insert(5, 0), Some(10));
        assert_eq!(m.get(&5), Some(&0));
        assert_eq!(*m.get_or_insert_with(7, || 99), 14);
        let fresh = m.get_or_insert_with(2_000, || 99);
        assert_eq!(*fresh, 99);
        *fresh += 1;
        assert_eq!(m.get(&2_000), Some(&100));
        assert_eq!(m.len(), 1_001);
        assert_eq!(format!("{m:?}"), "Lookup { len: 1001 }");

        // Borrowed keys: a String-keyed Lookup answers a &str.
        let mut names: Lookup<String, u32> = Lookup::new();
        names.insert("edge/3".to_string(), 0);
        assert_eq!(names.get("edge/3"), Some(&0));
        assert!(names.contains_key("edge/3"));
        assert_eq!(names.get("edg/3"), None);
    }
}
