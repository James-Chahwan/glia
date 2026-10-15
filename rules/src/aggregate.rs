//! Grouped count / min / max / sum as a stratum operator (DA.8).
//!
//! An aggregate rule `fan(y, count(x)) :- edge(x, y, "CALLS").` is not evaluated
//! round by round: its stratum runs the body ONCE over the complete lower strata
//! (DA.14b runs the plan at every level), projects each binding to
//! `[group columns.., aggregated columns..]` with the binding's level (the weakest
//! of its body rows) and height, and hands the lot to [`aggregate`].
//!
//! Semantics, the plan's binding decisions:
//! - **Set semantics (Q4).** One projected binding is one derivation, so duplicates
//!   collapse first, keeping the best (level, height): the strongest level, then the
//!   lowest height at it. Each op then ranges over the group's DISTINCT tuples of its
//!   own variables: `count(x, y)` counts distinct pairs and `sum(w)` sums distinct
//!   values of `w` (two edges of weight 3 sum to 3). This is unlike openCypher's bag
//!   `count`; a front end that wants a per-edge total aggregates over a variable that
//!   tells the edges apart.
//! - **Tier (Q5).** An output row takes the WEAKEST level among its group's
//!   (deduplicated) bindings: every one of them is in the set the op ranges over.
//! - **Height.** 1 + the highest height in the group, as for any derived row.
//! - **No zero.** Empty input yields no rows, and a group exists only because some
//!   binding put it there, so `count` is never 0. Absence is a negation.
//! - **Order.** `min` / `max` order values by the caller's comparator. DA.14b passes
//!   `ValueTable::cmp_rendered`, so a node ranks by its render key and the answer is
//!   checkout-stable; handle order (interning order) never picks the winner. The
//!   comparator takes the table as an argument because `cmp_rendered` fills the
//!   render cache through `&mut self`: [`aggregate`] lends it the one `&mut
//!   ValueTable` it holds, so the caller pre-renders nothing (the DA.3 handoff).
//! - **Checked sums.** `sum` adds i64 with overflow checks; a non-Int value or an
//!   overflow is a [`RulesError::Type`] naming the predicate.
//!
//! Output rows are sorted by group handles (internal order: `Evaluation::rows`
//! re-sorts by rendered values). Grouping is by sort, not by hash: no map is built.

use std::cmp::Ordering;

use crate::diag::RulesError;
use crate::ir::AggOp;
use crate::value::{Handle, Tier, ValueRef, ValueTable};

/// Where one head column of an aggregate rule comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutCol {
    /// Group column `i` of the projected binding (`i < AggSpec::group`). A head
    /// constant is projected as a group column holding the same handle throughout.
    Group(usize),
    /// The result of op `j` (`j < AggSpec::ops.len()`).
    Agg(usize),
}

/// One aggregate rule's shape over its projected bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AggSpec {
    /// The number of leading group columns.
    pub(crate) group: usize,
    /// Each op and the binding columns it aggregates (indexes into the projected
    /// row): count takes one or more, min / max / sum exactly one.
    pub(crate) ops: Vec<(AggOp, Vec<usize>)>,
    /// The head's column order.
    pub(crate) out_order: Vec<OutCol>,
}

/// The aggregate's rows, ready for `Relation::load`: `rows` is flat, one row per
/// `out_order.len()` handles, with each row's level and height alongside.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AggOutput {
    pub(crate) rows: Vec<Handle>,
    pub(crate) levels: Vec<Tier>,
    pub(crate) heights: Vec<u32>,
}

/// Flat bindings read as rows of `width` handles.
#[derive(Clone, Copy)]
struct Rows<'a> {
    data: &'a [Handle],
    width: usize,
}

impl<'a> Rows<'a> {
    fn get(self, i: usize) -> &'a [Handle] {
        &self.data[i * self.width..(i + 1) * self.width]
    }
}

fn type_error(pred: &str, message: String) -> RulesError {
    RulesError::Type { predicate: pred.to_string(), message }
}

/// Evaluate `spec` over `bindings` (flat, `width` handles per binding, with one
/// level and one height each). `cmp` orders two values for min / max and receives
/// `values` to do it with; `pred` (`name/arity`) names the rule in every error. A
/// spec the input cannot satisfy is an error before any row is read, so it fails
/// the same way on empty input.
#[allow(clippy::too_many_arguments)] // the spec's signature: three parallel input columns plus the table, order and name
pub(crate) fn aggregate(
    bindings: Vec<Handle>,
    width: usize,
    levels: Vec<Tier>,
    heights: Vec<u32>,
    spec: &AggSpec,
    values: &mut ValueTable,
    cmp: &mut dyn FnMut(&mut ValueTable, Handle, Handle) -> Ordering,
    pred: &str,
) -> Result<AggOutput, RulesError> {
    check(spec, width, bindings.len(), levels.len(), heights.len(), pred)?;
    let rows = Rows { data: &bindings, width };

    // (1) One derivation per projected row: sort by (row, level, height) and keep
    // the first of each run, the best level and its lowest height.
    let mut order: Vec<usize> = (0..levels.len()).collect();
    order.sort_unstable_by(|&a, &b| {
        rows.get(a)
            .cmp(rows.get(b))
            .then(levels[a].cmp(&levels[b]))
            .then(heights[a].cmp(&heights[b]))
    });
    order.dedup_by(|later, kept| rows.get(*later) == rows.get(*kept));

    // (2) Sorted rows put each group's bindings in one contiguous run.
    let mut out = AggOutput::default();
    let mut results: Vec<Handle> = Vec::with_capacity(spec.ops.len());
    let mut scratch: Vec<usize> = Vec::new();
    let mut start = 0;
    while start < order.len() {
        let key = &rows.get(order[start])[..spec.group];
        let mut end = start + 1;
        while end < order.len() && &rows.get(order[end])[..spec.group] == key {
            end += 1;
        }
        let members = &order[start..end];

        // (3) Each op over the group's distinct tuples of its own columns.
        results.clear();
        for (op, cols) in &spec.ops {
            results.push(apply(*op, cols, members, rows, values, cmp, &mut scratch, pred)?);
        }

        // (4) The head row in head order, the weakest level and 1 + the max height.
        for col in &spec.out_order {
            out.rows.push(match *col {
                OutCol::Group(i) => key[i],
                OutCol::Agg(j) => results[j],
            });
        }
        out.levels.push(members.iter().fold(Tier::Fact, |t, &i| t.weakest(levels[i])));
        let height = members.iter().map(|&i| heights[i]).max().unwrap_or(0);
        out.heights.push(height.saturating_add(1));
        start = end;
    }
    Ok(out)
}

/// One op's result over one group; `members` is non-empty (a group exists because a
/// binding put it there).
#[allow(clippy::too_many_arguments)]
fn apply(
    op: AggOp,
    cols: &[usize],
    members: &[usize],
    rows: Rows<'_>,
    values: &mut ValueTable,
    cmp: &mut dyn FnMut(&mut ValueTable, Handle, Handle) -> Ordering,
    scratch: &mut Vec<usize>,
    pred: &str,
) -> Result<Handle, RulesError> {
    match (op, cols) {
        (AggOp::Count, _) => {
            let n = distinct(members, cols, rows, scratch).len();
            let n = i64::try_from(n)
                .map_err(|_| type_error(pred, format!("count overflows i64 in `{pred}`")))?;
            Ok(values.intern(ValueRef::Int(n)))
        }
        (AggOp::Sum, &[col]) => {
            let mut total: i64 = 0;
            for &i in distinct(members, cols, rows, scratch) {
                let ValueRef::Int(x) = values.get(rows.get(i)[col]) else {
                    return Err(type_error(
                        pred,
                        format!("sum over a non-integer value in `{pred}`"),
                    ));
                };
                total = total
                    .checked_add(x)
                    .ok_or_else(|| type_error(pred, format!("sum overflows i64 in `{pred}`")))?;
            }
            Ok(values.intern(ValueRef::Int(total)))
        }
        (AggOp::Min | AggOp::Max, &[col]) => {
            let wins = if op == AggOp::Min { Ordering::Less } else { Ordering::Greater };
            let mut best = rows.get(members[0])[col];
            for &i in &members[1..] {
                let h = rows.get(i)[col];
                // A tie keeps the earlier value; cmp_rendered ties only equal handles.
                if h != best && cmp(values, h, best) == wins {
                    best = h;
                }
            }
            Ok(best)
        }
        // `check` admits min / max / sum over exactly one column.
        (AggOp::Min | AggOp::Max | AggOp::Sum, _) => {
            Err(type_error(pred, format!("`{op}` takes one variable in `{pred}`")))
        }
    }
}

/// The members whose `cols` tuples are distinct, one per tuple, in `scratch`.
fn distinct<'s>(
    members: &[usize],
    cols: &[usize],
    rows: Rows<'_>,
    scratch: &'s mut Vec<usize>,
) -> &'s [usize] {
    let tuple = |i: usize| cols.iter().map(move |&c| rows.get(i)[c]);
    scratch.clear();
    scratch.extend_from_slice(members);
    scratch.sort_unstable_by(|&a, &b| tuple(a).cmp(tuple(b)));
    scratch.dedup_by(|later, kept| tuple(*later).eq(tuple(*kept)));
    scratch
}

/// The spec fits the input: `cells` handles are `n` rows of `width`, every column
/// an op or the head names exists, and every op has the variables it takes. The
/// op-arity texts are reachable from rule text (the IR round-trips `sum(n, m)`); the
/// rest guard the caller's projection.
fn check(
    spec: &AggSpec,
    width: usize,
    cells: usize,
    n: usize,
    n_heights: usize,
    pred: &str,
) -> Result<(), RulesError> {
    let bad = |m: String| Err(type_error(pred, m));
    if n_heights != n || Some(cells) != n.checked_mul(width) {
        return bad(format!(
            "aggregate input for `{pred}` is malformed: {cells} values, {n} levels and \
             {n_heights} heights for rows of width {width}"
        ));
    }
    if spec.group > width {
        return bad(format!("aggregate of `{pred}` groups by {} of {width} columns", spec.group));
    }
    if spec.ops.is_empty() {
        return bad(format!("aggregate of `{pred}` has no aggregate in its head"));
    }
    for (op, cols) in &spec.ops {
        match (op, cols.len()) {
            (AggOp::Count, 0) => {
                return bad(format!("`count` takes at least one variable in `{pred}`"));
            }
            (AggOp::Min | AggOp::Max | AggOp::Sum, k) if k != 1 => {
                return bad(format!("`{op}` takes one variable, found {k} in `{pred}`"));
            }
            _ => {}
        }
        if let Some(c) = cols.iter().find(|&&c| c >= width) {
            return bad(format!("aggregate of `{pred}` reads column {c} of {width}"));
        }
    }
    for col in &spec.out_order {
        let fits = match *col {
            OutCol::Group(i) => i < spec.group,
            OutCol::Agg(j) => j < spec.ops.len(),
        };
        if !fits {
            return bad(format!("aggregate of `{pred}` names a head column it lacks: {col:?}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::NodeId;

    /// Handle order: the natural order of a test with no rendered values.
    fn by_handle(_: &mut ValueTable, a: Handle, b: Handle) -> Ordering {
        a.cmp(&b)
    }

    fn spec(group: usize, ops: &[(AggOp, &[usize])], out_order: &[OutCol]) -> AggSpec {
        AggSpec {
            group,
            ops: ops.iter().map(|(op, cols)| (*op, cols.to_vec())).collect(),
            out_order: out_order.to_vec(),
        }
    }

    /// `rows` interned as strings (or `#n` as Int n), each at its level and height.
    fn input(
        t: &mut ValueTable,
        rows: &[(&[&str], Tier, u32)],
    ) -> (Vec<Handle>, Vec<Tier>, Vec<u32>) {
        let mut b = Vec::new();
        for (row, _, _) in rows {
            for v in *row {
                b.push(match v.strip_prefix('#').map(str::parse::<i64>) {
                    Some(Ok(i)) => t.intern(ValueRef::Int(i)),
                    _ => t.intern(ValueRef::Str(v)),
                });
            }
        }
        (b, rows.iter().map(|r| r.1).collect(), rows.iter().map(|r| r.2).collect())
    }

    fn run(
        t: &mut ValueTable,
        rows: &[(&[&str], Tier, u32)],
        s: &AggSpec,
    ) -> Result<AggOutput, RulesError> {
        let width = rows.first().map_or(0, |r| r.0.len());
        let (b, l, h) = input(t, rows);
        aggregate(b, width, l, h, s, t, &mut by_handle, "w/2")
    }

    /// The output rows as rendered text (`#n` for an int).
    fn text(t: &ValueTable, out: &AggOutput, arity: usize) -> Vec<Vec<String>> {
        out.rows
            .chunks(arity)
            .map(|r| {
                r.iter()
                    .map(|&h| match t.get(h) {
                        ValueRef::Int(i) => format!("#{i}"),
                        ValueRef::Str(s) => s.to_string(),
                        ValueRef::Node(n) => format!("n{}", n.0),
                    })
                    .collect()
            })
            .collect()
    }

    fn err_text(r: Result<AggOutput, RulesError>) -> String {
        match r {
            Err(e @ RulesError::Type { .. }) => {
                assert!(matches!(&e, RulesError::Type { predicate, .. } if predicate == "w/2"));
                e.to_string()
            }
            other => panic!("expected a type error, got {other:?}"),
        }
    }

    const F: Tier = Tier::Fact;
    const D: Tier = Tier::Derived;
    const H: Tier = Tier::Heuristic;
    const COUNT_HEAD: [OutCol; 2] = [OutCol::Group(0), OutCol::Agg(0)];

    #[test]
    fn count_is_distinct() {
        let mut t = ValueTable::new();
        let s = spec(1, &[(AggOp::Count, &[1])], &COUNT_HEAD);
        let rows: &[(&[&str], Tier, u32)] = &[
            (&["g1", "x1"], F, 0),
            (&["g1", "x1"], F, 0),
            (&["g1", "x2"], F, 0),
            (&["g2", "x1"], F, 0),
        ];
        let out = run(&mut t, rows, &s).expect("count");
        assert_eq!(text(&t, &out, 2), [["g1", "#2"], ["g2", "#1"]]);

        // Over two columns a count counts distinct pairs; over one, distinct values
        // of that column alone.
        let s = spec(
            1,
            &[(AggOp::Count, &[1, 2]), (AggOp::Count, &[1])],
            &[OutCol::Group(0), OutCol::Agg(0), OutCol::Agg(1)],
        );
        let rows: &[(&[&str], Tier, u32)] = &[
            (&["g", "x1", "y1"], F, 0),
            (&["g", "x1", "y2"], F, 0),
            (&["g", "x1", "y1"], F, 0),
            (&["g", "x2", "y1"], F, 0),
        ];
        let out = run(&mut t, rows, &s).expect("count pairs");
        assert_eq!(text(&t, &out, 3), [["g", "#3", "#2"]]);
    }

    #[test]
    fn min_max_use_the_comparator() {
        let mut t = ValueTable::new();
        let head = [OutCol::Group(0), OutCol::Agg(0), OutCol::Agg(1)];
        let s = spec(1, &[(AggOp::Min, &[1]), (AggOp::Max, &[1])], &head);
        let rows: &[(&[&str], Tier, u32)] =
            &[(&["g", "#2"], F, 0), (&["g", "#1"], F, 0), (&["g", "#3"], F, 0)];
        let (b, l, h) = input(&mut t, rows);
        let out = aggregate(b.clone(), 2, l.clone(), h.clone(), &s, &mut t, &mut by_handle, "w/2")
            .expect("handle order");
        // Interned 2, 1, 3: handle order ranks 2 first and 3 last.
        assert_eq!(text(&t, &out, 3), [["g", "#2", "#3"]]);
        let mut reversed = |_: &mut ValueTable, a: Handle, b: Handle| b.cmp(&a);
        let out = aggregate(b, 2, l, h, &s, &mut t, &mut reversed, "w/2").expect("reversed");
        assert_eq!(text(&t, &out, 3), [["g", "#3", "#2"]]);
    }

    #[test]
    fn min_max_by_rendered_key_through_one_borrow() {
        // b's NodeId (100) is below a's (900) and b is interned first, yet a renders
        // first: the comparator, not the id or the handle, picks the winner.
        let render = |n: NodeId| {
            if n.0 == 900 { "a::x".to_string() } else { "b::y".to_string() }
        };
        let mut t = ValueTable::new();
        let g = t.intern(ValueRef::Str("g"));
        let nb = t.intern(ValueRef::Node(NodeId(100)));
        let na = t.intern(ValueRef::Node(NodeId(900)));
        let head = [OutCol::Group(0), OutCol::Agg(0), OutCol::Agg(1)];
        let s = spec(1, &[(AggOp::Min, &[1]), (AggOp::Max, &[1])], &head);
        let mut rendered = |t: &mut ValueTable, a: Handle, b: Handle| t.cmp_rendered(a, b, &render);
        let out = aggregate(
            vec![g, nb, g, na],
            2,
            vec![F, F],
            vec![0, 0],
            &s,
            &mut t,
            &mut rendered,
            "w/2",
        )
        .expect("rendered");
        assert_eq!(out.rows, [g, na, nb]);
    }

    #[test]
    fn sum_checks() {
        let mut t = ValueTable::new();
        let s = spec(1, &[(AggOp::Sum, &[1])], &COUNT_HEAD);
        let ok: &[(&[&str], Tier, u32)] =
            &[(&["g", "#1"], F, 0), (&["g", "#2"], F, 0), (&["g", "#3"], F, 0)];
        let out = run(&mut t, ok, &s).expect("sum");
        assert_eq!(text(&t, &out, 2), [["g", "#6"]]);

        // Set semantics over the aggregated variable: w = 3 on two edges sums once.
        let edges = spec(1, &[(AggOp::Sum, &[2])], &COUNT_HEAD);
        let twice: &[(&[&str], Tier, u32)] =
            &[(&["g", "e1", "#3"], F, 0), (&["g", "e2", "#3"], F, 0)];
        let out = run(&mut t, twice, &edges).expect("sum distinct");
        assert_eq!(text(&t, &out, 2), [["g", "#3"]]);

        let mixed: &[(&[&str], Tier, u32)] = &[(&["g", "#1"], F, 0), (&["g", "x"], F, 0)];
        assert_eq!(err_text(run(&mut t, mixed, &s)), "sum over a non-integer value in `w/2`");

        let mut t = ValueTable::new();
        let max = t.intern(ValueRef::Int(i64::MAX));
        let one = t.intern(ValueRef::Int(1));
        let g = t.intern(ValueRef::Str("g"));
        let r = aggregate(
            vec![g, max, g, one],
            2,
            vec![F, F],
            vec![0, 0],
            &s,
            &mut t,
            &mut by_handle,
            "w/2",
        );
        assert_eq!(err_text(r), "sum overflows i64 in `w/2`");
    }

    #[test]
    fn tier_is_the_weakest_row() {
        let mut t = ValueTable::new();
        let s = spec(1, &[(AggOp::Count, &[1])], &COUNT_HEAD);
        let rows: &[(&[&str], Tier, u32)] = &[
            (&["g1", "x1"], F, 0),
            (&["g1", "x2"], H, 0),
            (&["g2", "x1"], F, 0),
            (&["g2", "x2"], F, 0),
            // One binding derived twice: it counts once, at its best level.
            (&["g3", "x1"], D, 0),
            (&["g3", "x1"], F, 0),
        ];
        let out = run(&mut t, rows, &s).expect("tiers");
        assert_eq!(text(&t, &out, 2), [["g1", "#2"], ["g2", "#2"], ["g3", "#1"]]);
        assert_eq!(out.levels, [H, F, F]);
    }

    #[test]
    fn height_is_one_plus_max() {
        let mut t = ValueTable::new();
        let s = spec(1, &[(AggOp::Count, &[1])], &COUNT_HEAD);
        let rows: &[(&[&str], Tier, u32)] = &[
            (&["g1", "x1"], F, 0),
            (&["g1", "x2"], F, 4),
            (&["g1", "x3"], F, 2),
            // A duplicate keeps its lowest height ...
            (&["g2", "x1"], F, 7),
            (&["g2", "x1"], F, 2),
            // ... at its best level: the FACT derivation's 6, not the DERIVED one's 1.
            (&["g3", "x1"], D, 1),
            (&["g3", "x1"], F, 6),
        ];
        let out = run(&mut t, rows, &s).expect("heights");
        assert_eq!(out.heights, [5, 3, 7]);
        assert_eq!(out.levels, [F, F, F]);
    }

    #[test]
    fn empty_input_no_rows() {
        let mut t = ValueTable::new();
        let s = spec(1, &[(AggOp::Count, &[1])], &COUNT_HEAD);
        let out =
            aggregate(Vec::new(), 2, Vec::new(), Vec::new(), &s, &mut t, &mut by_handle, "w/2")
                .expect("empty");
        assert_eq!(out, AggOutput::default());
        assert_eq!(t.len(), 0, "no count of 0 is interned");
    }

    #[test]
    fn multiple_ops_one_head() {
        // stats(g, count(x), max(x)), and the same ops in another head order.
        let mut t = ValueTable::new();
        let rows: &[(&[&str], Tier, u32)] = &[
            (&["g1", "#5"], F, 0),
            (&["g1", "#9"], D, 1),
            (&["g1", "#5"], F, 3),
            (&["g2", "#4"], F, 0),
        ];
        let ops: &[(AggOp, &[usize])] = &[(AggOp::Count, &[1]), (AggOp::Max, &[1])];
        let s = spec(1, ops, &[OutCol::Group(0), OutCol::Agg(0), OutCol::Agg(1)]);
        let out = run(&mut t, rows, &s).expect("stats");
        assert_eq!(text(&t, &out, 3), [["g1", "#2", "#9"], ["g2", "#1", "#4"]]);
        assert_eq!(out.levels, [D, F]);
        assert_eq!(out.heights, [2, 1]);
        let s = spec(1, ops, &[OutCol::Agg(1), OutCol::Group(0), OutCol::Agg(0)]);
        let out = run(&mut t, rows, &s).expect("reordered");
        assert_eq!(text(&t, &out, 3), [["#9", "g1", "#2"], ["#4", "g2", "#1"]]);

        // No group columns: one row over every binding.
        let s = spec(0, &[(AggOp::Count, &[1])], &[OutCol::Agg(0)]);
        let out = run(&mut t, rows, &s).expect("ungrouped");
        assert_eq!(text(&t, &out, 1), [["#3"]]);
    }

    #[test]
    fn spec_errors_name_the_predicate() {
        let mut t = ValueTable::new();
        let mut empty = |s: &AggSpec, width: usize| {
            aggregate(Vec::new(), width, Vec::new(), Vec::new(), s, &mut t, &mut by_handle, "w/2")
        };
        // sum(n, m) and count() parse and analyse; the operator rejects them, even
        // with no input.
        let two = spec(1, &[(AggOp::Sum, &[1, 2])], &COUNT_HEAD);
        assert_eq!(err_text(empty(&two, 3)), "`sum` takes one variable, found 2 in `w/2`");
        let none = spec(1, &[(AggOp::Count, &[])], &COUNT_HEAD);
        assert_eq!(err_text(empty(&none, 1)), "`count` takes at least one variable in `w/2`");
        let wide = spec(1, &[(AggOp::Max, &[2])], &COUNT_HEAD);
        assert_eq!(err_text(empty(&wide, 2)), "aggregate of `w/2` reads column 2 of 2");
        let head = spec(1, &[(AggOp::Count, &[1])], &[OutCol::Group(1)]);
        assert_eq!(
            err_text(empty(&head, 2)),
            "aggregate of `w/2` names a head column it lacks: Group(1)"
        );
        let count = spec(1, &[(AggOp::Count, &[1])], &COUNT_HEAD);
        let r =
            aggregate(vec![0, 1, 2], 2, vec![F], vec![0], &count, &mut t, &mut by_handle, "w/2");
        assert_eq!(
            err_text(r),
            "aggregate input for `w/2` is malformed: 3 values, 1 levels and 1 heights for rows of width 2"
        );
    }
}
