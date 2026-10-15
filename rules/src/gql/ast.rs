//! The parsed GQL read subset: what [`super::parse`] returns and DB.7b lowers to the
//! rule IR. The tree is domain-free: labels, relationship types, property keys and
//! function names are plain identifiers, resolved later against a FactSource's
//! registries. Every node carries the [`Span`] of the text it was read from (byte
//! offsets, 1-based line and char column; DA's lexer), so the lowering can locate
//! its diagnostics; a column is for display only, never for slicing.
//!
//! Quantifiers are normalised at parse time, so every spelling of one bound reads
//! back as one [`Quant`]: Cypher `*` / `*1..` / GQL `+` / `{1,}` are all
//! `{ min: 1, max: None }`. Operators reuse the IR's [`CmpOp`] and [`AggOp`].

use crate::diag::Span;
use crate::ir::{AggOp, CmpOp};

/// A whole query: `MATCH .. [WHERE ..] RETURN [DISTINCT] .. [ORDER BY ..] [LIMIT n]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// The MATCH clauses in order; one or more. Without OPTIONAL MATCH their
    /// patterns are one conjunction.
    pub matches: Vec<MatchClause>,
    /// The WHERE condition, when written.
    pub condition: Option<Expr>,
    /// `RETURN DISTINCT`: accepted, and the same as RETURN under set semantics.
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
    pub order_by: Vec<OrderKey>,
    pub limit: Option<u64>,
    pub span: Span,
}

/// One `MATCH p1, p2, ..` clause.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchClause {
    pub patterns: Vec<PathPattern>,
    pub span: Span,
}

/// An identifier as written (case kept) and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// `[p = [ANY SHORTEST]] (a) -[..]-> (b) ...`: a node, then (relationship, node)
/// steps. A pattern inside WHERE (or `EXISTS { .. }`) has no path variable.
#[derive(Clone, Debug, PartialEq)]
pub struct PathPattern {
    /// The path variable `p` of `p = ...`.
    pub var: Option<Ident>,
    /// `ANY SHORTEST`: the row's path is one shortest witness.
    pub shortest: bool,
    pub start: NodePat,
    pub steps: Vec<(RelPat, NodePat)>,
    pub span: Span,
}

/// `(v:L1|L2 {k: lit, ..})`; every part optional. Two labels are alternatives.
#[derive(Clone, Debug, PartialEq)]
pub struct NodePat {
    pub var: Option<Ident>,
    pub labels: Vec<Ident>,
    pub props: Vec<(Ident, Lit)>,
    pub span: Span,
}

/// `-[r:T1|T2 *m..n]->`, `<-[..]-`, `-[..]-`, or the abbreviated `-->`, `<--`, `--`
/// (no variable, no type). Several types are alternatives.
#[derive(Clone, Debug, PartialEq)]
pub struct RelPat {
    pub var: Option<Ident>,
    pub types: Vec<Ident>,
    pub dir: Dir,
    /// The repetition bounds, from either spelling (never both); `None` is one hop.
    pub quant: Option<Quant>,
    pub span: Span,
}

/// Which way a relationship pattern points, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// `-[..]->`: from the left node to the right one.
    Out,
    /// `<-[..]-`: from the right node to the left one.
    In,
    /// `-[..]-`: either way.
    Undirected,
}

/// Repetition bounds in hops: `max: None` is unbounded. `min <= max` always holds,
/// and a typed relationship never has `max: Some(0)` (the parser refuses both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quant {
    pub min: u32,
    pub max: Option<u32>,
    /// The quantifier's text: `*m..n` inside the brackets or `+` / `*` / `{m,n}`
    /// after them.
    pub span: Span,
}

impl Quant {
    /// The bounds alone, without the span: equal for every spelling of one bound.
    pub fn bounds(&self) -> (u32, Option<u32>) {
        (self.min, self.max)
    }
}

/// A WHERE condition. `NOT` binds tighter than `AND`, which binds tighter than `OR`;
/// the binary forms are left-associative.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Or {
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    And {
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Not {
        expr: Box<Expr>,
        span: Span,
    },
    /// `left op right` (`=`, `<>` / `!=`, `<`, `<=`, `>`, `>=`).
    Cmp {
        op: CmpOp,
        left: Operand,
        right: Operand,
        span: Span,
    },
    /// `left STARTS WITH right`, `ENDS WITH`, `CONTAINS`.
    StrTest {
        op: StrOp,
        left: Operand,
        right: Operand,
        span: Span,
    },
    /// `left IN [lit, ..]`.
    In {
        left: Operand,
        list: Vec<Lit>,
        span: Span,
    },
    /// A pattern predicate, bare `(a)-[:T]->()` or `EXISTS { (a)-[:T]->() }`
    /// (`exists`); both mean "some match exists".
    Pattern {
        pattern: Box<PathPattern>,
        exists: bool,
        span: Span,
    },
    /// A boolean function call, `in_scope(a, 'web')`: the builtin of that name.
    Call {
        name: Ident,
        args: Vec<Operand>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Or { span, .. }
            | Expr::And { span, .. }
            | Expr::Not { span, .. }
            | Expr::Cmp { span, .. }
            | Expr::StrTest { span, .. }
            | Expr::In { span, .. }
            | Expr::Pattern { span, .. }
            | Expr::Call { span, .. } => *span,
        }
    }
}

/// The string tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrOp {
    StartsWith,
    EndsWith,
    Contains,
}

/// A value in a condition, a return item or an ORDER BY key.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    /// A node, relationship or path variable.
    Var(Ident),
    /// `var.key`: an attribute of the node bound to `var`.
    Prop {
        var: Ident,
        key: Ident,
        span: Span,
    },
    Lit(Lit),
    /// `type(r)`: the relationship variable's type (edge category).
    Type {
        var: Ident,
        span: Span,
    },
}

impl Operand {
    pub fn span(&self) -> Span {
        match self {
            Operand::Var(v) => v.span,
            Operand::Prop { span, .. } | Operand::Type { span, .. } => *span,
            Operand::Lit(l) => l.span(),
        }
    }
}

/// A literal: `'..'` / `".."` with escapes applied, an integer (a leading `-`
/// folded in), `TRUE` / `FALSE`.
#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Str { value: String, span: Span },
    Int { value: i64, span: Span },
    Bool { value: bool, span: Span },
}

impl Lit {
    pub fn span(&self) -> Span {
        match self {
            Lit::Str { span, .. } | Lit::Int { span, .. } | Lit::Bool { span, .. } => *span,
        }
    }
}

/// One RETURN item: a value or an aggregate, with its `AS` alias.
#[derive(Clone, Debug, PartialEq)]
pub struct ReturnItem {
    pub expr: RetExpr,
    pub alias: Option<Ident>,
    pub span: Span,
}

/// What a RETURN item computes.
#[derive(Clone, Debug, PartialEq)]
pub enum RetExpr {
    Operand(Operand),
    Agg(Agg),
}

/// `count(*)`, `count([DISTINCT] x)`, `min(x)`, `max(x)`, `sum(x)`; `arg: None` is
/// `count(*)`. DISTINCT is kept as written; under set semantics it changes nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Agg {
    pub op: AggOp,
    pub distinct: bool,
    pub arg: Option<Operand>,
    pub span: Span,
}

/// One ORDER BY key: a column alias or `var.key` ([`Operand::Var`] or
/// [`Operand::Prop`]), ascending unless `DESC`.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderKey {
    pub key: Operand,
    pub descending: bool,
    pub span: Span,
}
