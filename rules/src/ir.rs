//! The string-named rule IR (Program / Rule / Atom / Term / Aggregate / Comparison)
//! and the [`Catalog`] of the predicates and builtins a program may read.
//!
//! The IR is the contract two front ends build: the Datalog parser
//! (`parser::parse_program`) and the GQL read subset's lowering (`gql`). Every name
//! in it is a STRING: predicates, variables and the kind / category / cell-type
//! constants a rule compares against (`node(r, "ROUTE")`). Names resolve to the
//! domain's registry ids only in analysis, where an unknown name is an error naming
//! it, so a rule stays readable while ids stay locked per domain.
//!
//! The structs are exhaustive with pub fields so a front end builds them by literal;
//! the constructors (`Term::var`, `Atom::new`, `Literal::pos`, `Rule::new`,
//! `Rule::fact`, ...) fill every span with `Span::default()`, for programmatic IR
//! and tests. A rule with an empty body is a fact.
//!
//! Display renders canonical Datalog, the text the parser reads back to an equal IR
//! modulo spans: one clause per line, `head :- lit, lit.`; a negated atom is `!p(x)`;
//! a comparison is `left op right` with spaces (so `a < -1` never lexes as `<-`); a
//! body atom always takes parentheses (`p()` at arity 0, since a bare identifier in a
//! body starts a comparison) while an arity-0 head is its bare name; strings are
//! double-quoted with `\\`, `\"`, `\n` and `\t` escaped. Variable names render as
//! given, so the round trip holds for names that are identifiers.
//!
//! The [`Catalog`] is the analysis-time declaration set, built without a graph so a
//! rule can be checked at load: the core EDB predicates every domain gets, the
//! domain's attributes (`name(Node, ty)`) and the builtins (the core `reaches` /
//! `scc` / string tests, plus the domain's own).

use std::fmt;

use glia_activation::profile::Registries;
use glia_core::EdgeCategoryId;

use crate::diag::Span;

/// The index of a source in [`Program::sources`]; spans carry it.
pub type SourceId = u16;

/// The name [`Program::source_name`] gives a source id the program does not hold
/// (programmatic IR whose spans are all `Span::default()`).
pub const UNNAMED_SOURCE: &str = "<program>";

// ============================================================================
// The rule IR
// ============================================================================

/// A rule program: the named sources its spans point into, and its rules in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub sources: Vec<String>,
    pub rules: Vec<Rule>,
}

impl Program {
    /// An empty program with no source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a source named `name` and return its id. Every call adds a source,
    /// even for a name already held: two stanzas may share a name and stay apart.
    /// Past `u16::MAX` sources the id saturates, so later sources share the last id.
    pub fn add_source(&mut self, name: &str) -> SourceId {
        let id = SourceId::try_from(self.sources.len()).unwrap_or(SourceId::MAX);
        self.sources.push(name.to_string());
        id
    }

    /// Move `other`'s sources and rules onto the end of this program. Every span in
    /// `other`'s rules is remapped to the source's new id, and every source name is
    /// kept (no deduplication).
    pub fn append(&mut self, other: Program) {
        let offset = SourceId::try_from(self.sources.len()).unwrap_or(SourceId::MAX);
        self.sources.extend(other.sources);
        for mut rule in other.rules {
            rule.visit_spans(&mut |s: &mut Span| s.source = s.source.saturating_add(offset));
            self.rules.push(rule);
        }
    }

    /// The name of source `s`, or [`UNNAMED_SOURCE`] when the program holds no such
    /// source.
    pub fn source_name(&self, s: SourceId) -> &str {
        self.sources.get(usize::from(s)).map_or(UNNAMED_SOURCE, String::as_str)
    }
}

/// One clause: `head :- body.`; an empty body makes it a fact.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub head: Head,
    pub body: Vec<Literal>,
    pub span: Span,
}

impl Rule {
    /// A rule `head_pred(head_args) :- body.` with default spans.
    pub fn new(head_pred: impl Into<String>, head_args: Vec<HeadArg>, body: Vec<Literal>) -> Self {
        Rule {
            head: Head { pred: head_pred.into(), args: head_args, span: Span::default() },
            body,
            span: Span::default(),
        }
    }

    /// A fact `pred(consts).` with default spans.
    pub fn fact(pred: impl Into<String>, consts: Vec<Term>) -> Self {
        Self::new(pred, consts.into_iter().map(HeadArg::Term).collect(), Vec::new())
    }

    /// Is this clause a fact (no body)?
    pub fn is_fact(&self) -> bool {
        self.body.is_empty()
    }

    /// Apply `f` to every span the rule carries: its own, its head's, every head
    /// argument's, every literal's and every term's.
    fn visit_spans(&mut self, f: &mut dyn FnMut(&mut Span)) {
        f(&mut self.span);
        f(&mut self.head.span);
        for arg in &mut self.head.args {
            match arg {
                HeadArg::Term(t) => f(t.span_mut()),
                HeadArg::Agg(a) => f(&mut a.span),
            }
        }
        for lit in &mut self.body {
            match lit {
                Literal::Pos(a) | Literal::Neg(a) => {
                    f(&mut a.span);
                    for t in &mut a.args {
                        f(t.span_mut());
                    }
                }
                Literal::Cmp(c) => {
                    f(&mut c.span);
                    f(c.left.span_mut());
                    f(c.right.span_mut());
                }
            }
        }
    }
}

/// A rule's head: the predicate it defines and its arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub pred: String,
    pub args: Vec<HeadArg>,
    pub span: Span,
}

impl Head {
    /// The number of head arguments.
    pub fn arity(&self) -> usize {
        self.args.len()
    }

    /// `name/arity`, the predicate as diagnostics name it.
    pub fn signature(&self) -> String {
        format!("{}/{}", self.pred, self.args.len())
    }
}

/// One head argument: a term, or an aggregate over body variables.
#[derive(Clone, Debug, PartialEq)]
pub enum HeadArg {
    Term(Term),
    Agg(Aggregate),
}

impl HeadArg {
    /// An aggregate head argument `op(vars)` with a default span.
    pub fn agg(op: AggOp, vars: &[&str]) -> Self {
        HeadArg::Agg(Aggregate {
            op,
            vars: vars.iter().map(|v| (*v).to_string()).collect(),
            span: Span::default(),
        })
    }
}

impl From<Term> for HeadArg {
    fn from(t: Term) -> Self {
        HeadArg::Term(t)
    }
}

/// A head aggregate: `count(x)`, `min(x)`, `max(x)`, `sum(x)`; `vars` are the body
/// variables it ranges over.
#[derive(Clone, Debug, PartialEq)]
pub struct Aggregate {
    pub op: AggOp,
    pub vars: Vec<String>,
    pub span: Span,
}

/// The aggregate operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggOp {
    Count,
    Min,
    Max,
    Sum,
}

impl AggOp {
    /// The operator's surface name.
    pub fn name(self) -> &'static str {
        match self {
            AggOp::Count => "count",
            AggOp::Min => "min",
            AggOp::Max => "max",
            AggOp::Sum => "sum",
        }
    }
}

/// One body literal: a positive atom, a negated atom, or a comparison.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Pos(Atom),
    Neg(Atom),
    Cmp(Comparison),
}

impl Literal {
    /// A positive literal.
    pub fn pos(atom: Atom) -> Self {
        Literal::Pos(atom)
    }

    /// A negated literal `!atom`.
    pub fn neg(atom: Atom) -> Self {
        Literal::Neg(atom)
    }

    /// A comparison literal `left op right` with a default span.
    pub fn cmp(op: CmpOp, left: Term, right: Term) -> Self {
        Literal::Cmp(Comparison { op, left, right, span: Span::default() })
    }

    /// The literal's span.
    pub fn span(&self) -> Span {
        match self {
            Literal::Pos(a) | Literal::Neg(a) => a.span,
            Literal::Cmp(c) => c.span,
        }
    }
}

/// A predicate applied to terms: `edge(x, y, "CALLS")`.
#[derive(Clone, Debug, PartialEq)]
pub struct Atom {
    pub pred: String,
    pub args: Vec<Term>,
    pub span: Span,
}

impl Atom {
    /// An atom `pred(args)` with a default span.
    pub fn new(pred: impl Into<String>, args: Vec<Term>) -> Self {
        Atom { pred: pred.into(), args, span: Span::default() }
    }

    /// `name/arity`, the predicate as diagnostics name it.
    pub fn signature(&self) -> String {
        format!("{}/{}", self.pred, self.args.len())
    }
}

/// A term: a variable, the wildcard `_`, a string constant or an integer constant.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Var { name: String, span: Span },
    Wild(Span),
    Str { value: String, span: Span },
    Int { value: i64, span: Span },
}

impl Term {
    /// A variable with a default span.
    pub fn var(name: impl Into<String>) -> Self {
        Term::Var { name: name.into(), span: Span::default() }
    }

    /// The wildcard `_` with a default span.
    pub fn wild() -> Self {
        Term::Wild(Span::default())
    }

    /// A string constant with a default span.
    pub fn str(value: impl Into<String>) -> Self {
        Term::Str { value: value.into(), span: Span::default() }
    }

    /// An integer constant with a default span.
    pub fn int(value: i64) -> Self {
        Term::Int { value, span: Span::default() }
    }

    /// The term's span.
    pub fn span(&self) -> Span {
        match self {
            Term::Var { span, .. } | Term::Str { span, .. } | Term::Int { span, .. } => *span,
            Term::Wild(span) => *span,
        }
    }

    fn span_mut(&mut self) -> &mut Span {
        match self {
            Term::Var { span, .. } | Term::Str { span, .. } | Term::Int { span, .. } => span,
            Term::Wild(span) => span,
        }
    }
}

/// A comparison literal `left op right`.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub op: CmpOp,
    pub left: Term,
    pub right: Term,
    pub span: Span,
}

/// The comparison operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    /// The operator's canonical spelling (`Ne` is `!=`; the lexer also reads `<>`).
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

// ============================================================================
// Canonical Display
// ============================================================================

/// Write `value` as a double-quoted string with `\\`, `\"`, `\n` and `\t` escaped.
fn write_str_literal(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    f.write_str("\"")?;
    for c in value.chars() {
        match c {
            '\\' => f.write_str("\\\\")?,
            '"' => f.write_str("\\\"")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            c => write!(f, "{c}")?,
        }
    }
    f.write_str("\"")
}

/// Write `items` separated by `, `.
fn write_list<T: fmt::Display>(f: &mut fmt::Formatter<'_>, items: &[T]) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Var { name, .. } => f.write_str(name),
            Term::Wild(_) => f.write_str("_"),
            Term::Str { value, .. } => write_str_literal(f, value),
            Term::Int { value, .. } => write!(f, "{value}"),
        }
    }
}

impl fmt::Display for AggOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for Aggregate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.op)?;
        write_list(f, &self.vars)?;
        f.write_str(")")
    }
}

impl fmt::Display for HeadArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeadArg::Term(t) => write!(f, "{t}"),
            HeadArg::Agg(a) => write!(f, "{a}"),
        }
    }
}

impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.pred)?;
        if self.args.is_empty() {
            return Ok(());
        }
        f.write_str("(")?;
        write_list(f, &self.args)?;
        f.write_str(")")
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.pred)?;
        write_list(f, &self.args)?;
        f.write_str(")")
    }
}

impl fmt::Display for CmpOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.left, self.op, self.right)
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Pos(a) => write!(f, "{a}"),
            Literal::Neg(a) => write!(f, "!{a}"),
            Literal::Cmp(c) => write!(f, "{c}"),
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.head)?;
        if !self.body.is_empty() {
            f.write_str(" :- ")?;
            write_list(f, &self.body)?;
        }
        f.write_str(".")
    }
}

impl fmt::Display for Program {
    /// One clause per line, each ending in a newline.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for rule in &self.rules {
            writeln!(f, "{rule}")?;
        }
        Ok(())
    }
}

// ============================================================================
// Declarations: the Catalog
// ============================================================================

/// The type of one predicate column, as analysis checks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColType {
    /// A node of the graph.
    Node,
    /// A node-kind name from the domain's registry.
    Kind,
    /// An edge-category name from the domain's registry.
    Category,
    /// A cell-type name from the domain's registry.
    CellType,
    /// A `|`-separated set of edge-category names, or `*` for every category.
    CategorySet,
    Str,
    Int,
    Any,
}

/// What a builtin needs of one argument before it is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindReq {
    /// Bound or free.
    May,
    /// Bound by an earlier literal (or a constant).
    Must,
    /// A literal constant in the rule text.
    Const,
}

/// An EDB predicate: its name and column types.
#[derive(Clone, Debug, PartialEq)]
pub struct PredDecl {
    pub name: String,
    pub cols: Vec<ColType>,
}

impl PredDecl {
    /// `name/arity`.
    pub fn signature(&self) -> String {
        format!("{}/{}", self.name, self.cols.len())
    }
}

/// A domain attribute: the predicate `name(Node, ty)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttrDecl {
    pub name: &'static str,
    pub ty: ColType,
}

/// Which builtin implementation a declaration names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinKind {
    Reaches,
    Scc,
    StartsWith,
    EndsWith,
    Contains,
    /// A builtin the domain supplies.
    Domain,
}

/// A builtin predicate: its name, column types, what each argument needs bound
/// before the call, whether its rows carry an evidence tier, and its implementation.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinDecl {
    pub name: String,
    pub cols: Vec<ColType>,
    pub bind: Vec<BindReq>,
    pub tiered: bool,
    pub kind: BuiltinKind,
}

impl BuiltinDecl {
    /// `name/arity`.
    pub fn signature(&self) -> String {
        format!("{}/{}", self.name, self.cols.len())
    }
}

/// The predicates and builtins a program may read, with the domain's registries and
/// carry edges.
///
/// Names have one owner: an EDB predicate or attribute name is unique across EDB
/// predicates, attributes and builtins; a builtin is keyed by name and arity (the
/// core `reaches/2`, `/3`, `/4` share a name) and may not take an EDB name. A
/// declaration that collides with an earlier one is ignored, so the core wins.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    pub registries: Registries,
    /// The domain's carry edges (`carry_category/1`, and what `reaches/2` follows).
    pub carry: Vec<EdgeCategoryId>,
    pub edb: Vec<PredDecl>,
    pub builtins: Vec<BuiltinDecl>,
}

fn pred(name: &str, cols: &[ColType]) -> PredDecl {
    PredDecl { name: name.to_string(), cols: cols.to_vec() }
}

fn builtin(
    name: &str,
    cols: &[ColType],
    bind: &[BindReq],
    tiered: bool,
    kind: BuiltinKind,
) -> BuiltinDecl {
    BuiltinDecl { name: name.to_string(), cols: cols.to_vec(), bind: bind.to_vec(), tiered, kind }
}

impl Catalog {
    /// The core catalog every domain gets. EDB, in order: `node(Node, Kind)`,
    /// `edge(Node, Node, Category)`, `edge_conf(Node, Node, Category, Str)` (the
    /// confidence: strong / medium / weak), `cell(Node, CellType, Str)`,
    /// `edge_cell(Node, Node, Category, CellType, Str)`, `kind(Kind)`,
    /// `category(Category)`, `carry_category(Category)`. Builtins, in order:
    /// `reaches/2` (over `carry`), `reaches/3` (+ a constant category set, `"A|B"` or
    /// `"*"`), `reaches/4` (+ a constant max depth), `scc/2`, `scc/3` (+ a constant
    /// category set), all tiered; `starts_with/2`, `ends_with/2`, `contains/2` (both
    /// strings bound), untiered.
    pub fn core(registries: Registries, carry: &[EdgeCategoryId]) -> Self {
        use BindReq::{Const, May, Must};
        use ColType::{Category, CategorySet, CellType, Int, Kind, Node, Str};
        let edb = vec![
            pred("node", &[Node, Kind]),
            pred("edge", &[Node, Node, Category]),
            pred("edge_conf", &[Node, Node, Category, Str]),
            pred("cell", &[Node, CellType, Str]),
            pred("edge_cell", &[Node, Node, Category, CellType, Str]),
            pred("kind", &[Kind]),
            pred("category", &[Category]),
            pred("carry_category", &[Category]),
        ];
        let builtins = vec![
            builtin("reaches", &[Node, Node], &[May, May], true, BuiltinKind::Reaches),
            builtin(
                "reaches",
                &[Node, Node, CategorySet],
                &[May, May, Const],
                true,
                BuiltinKind::Reaches,
            ),
            builtin(
                "reaches",
                &[Node, Node, CategorySet, Int],
                &[May, May, Const, Const],
                true,
                BuiltinKind::Reaches,
            ),
            builtin("scc", &[Node, Node], &[May, May], true, BuiltinKind::Scc),
            builtin("scc", &[Node, Node, CategorySet], &[May, May, Const], true, BuiltinKind::Scc),
            builtin("starts_with", &[Str, Str], &[Must, Must], false, BuiltinKind::StartsWith),
            builtin("ends_with", &[Str, Str], &[Must, Must], false, BuiltinKind::EndsWith),
            builtin("contains", &[Str, Str], &[Must, Must], false, BuiltinKind::Contains),
        ];
        Catalog { registries, carry: carry.to_vec(), edb, builtins }
    }

    /// Add each attribute as the EDB predicate `name(Node, ty)`, after those already
    /// declared. An attribute whose name is already an EDB predicate, an attribute
    /// or a builtin is ignored.
    pub fn with_attrs(mut self, attrs: &[AttrDecl]) -> Self {
        for a in attrs {
            if self.edb(a.name).is_some() || self.builtins.iter().any(|b| b.name == a.name) {
                continue;
            }
            self.edb.push(pred(a.name, &[ColType::Node, a.ty]));
        }
        self
    }

    /// Add each builtin after those already declared. A builtin whose name is an
    /// EDB predicate, or whose name and arity are already a builtin, is ignored.
    pub fn with_builtins(mut self, decls: Vec<BuiltinDecl>) -> Self {
        for d in decls {
            if self.edb(&d.name).is_some() || self.builtin(&d.name, d.cols.len()).is_some() {
                continue;
            }
            self.builtins.push(d);
        }
        self
    }

    /// The EDB predicate (core or attribute) named `name`.
    pub fn edb(&self, name: &str) -> Option<&PredDecl> {
        self.edb.iter().find(|p| p.name == name)
    }

    /// The builtin named `name` with `arity` arguments.
    pub fn builtin(&self, name: &str, arity: usize) -> Option<&BuiltinDecl> {
        self.builtins.iter().find(|b| b.name == name && b.cols.len() == arity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::{CellTypeId, NodeKindId};

    const REG: Registries = Registries {
        node_kinds: &[(NodeKindId(1), "ROUTE"), (NodeKindId(2), "METHOD")],
        edge_categories: &[(EdgeCategoryId(1), "CALLS"), (EdgeCategoryId(2), "USES")],
        cell_types: &[(CellTypeId(1), "CODE")],
    };

    fn sp(source: SourceId, start: u32) -> Span {
        Span { source, start, end: start + 1, line: 1, col: start + 1 }
    }

    fn sources_of(rule: &Rule) -> Vec<SourceId> {
        let mut out = Vec::new();
        rule.clone().visit_spans(&mut |s: &mut Span| out.push(s.source));
        out
    }

    #[test]
    fn display_is_canonical() {
        let mut p = Program::new();
        p.add_source("rules");
        p.rules.push(Rule::fact("allowed", vec![Term::str("web"), Term::str("a\"b\\c\nd\te")]));
        p.rules.push(Rule::new(
            "v",
            vec![Term::var("x").into()],
            vec![
                Literal::pos(Atom::new("node", vec![Term::var("x"), Term::wild()])),
                Literal::neg(Atom::new("handled", vec![Term::var("x")])),
                Literal::cmp(CmpOp::Ne, Term::var("x"), Term::str("y")),
                Literal::pos(Atom::new("depth", vec![Term::var("x"), Term::var("n")])),
                Literal::cmp(CmpOp::Ge, Term::var("n"), Term::int(-2)),
                Literal::cmp(CmpOp::Lt, Term::var("n"), Term::int(i64::MIN)),
            ],
        ));
        p.rules.push(Rule::new(
            "fan_in",
            vec![Term::var("y").into(), HeadArg::agg(AggOp::Count, &["x"])],
            vec![Literal::pos(Atom::new(
                "edge",
                vec![Term::var("x"), Term::var("y"), Term::str("CALLS")],
            ))],
        ));
        p.rules.push(Rule::new(
            "ok",
            Vec::new(),
            vec![
                Literal::pos(Atom::new("done", Vec::new())),
                Literal::cmp(CmpOp::Eq, Term::int(1), Term::int(1)),
            ],
        ));
        p.rules.push(Rule::new(
            "w",
            vec![Term::var("a").into(), HeadArg::agg(AggOp::Sum, &["n", "m"])],
            vec![
                Literal::pos(Atom::new("p", vec![Term::var("a"), Term::var("n"), Term::var("m")])),
                Literal::cmp(CmpOp::Le, Term::var("n"), Term::int(3)),
                Literal::cmp(CmpOp::Gt, Term::var("m"), Term::int(0)),
            ],
        ));
        p.rules.push(Rule::fact("flag", Vec::new()));
        let expected = r#"allowed("web", "a\"b\\c\nd\te").
v(x) :- node(x, _), !handled(x), x != "y", depth(x, n), n >= -2, n < -9223372036854775808.
fan_in(y, count(x)) :- edge(x, y, "CALLS").
ok :- done(), 1 = 1.
w(a, sum(n, m)) :- p(a, n, m), n <= 3, m > 0.
flag.
"#;
        assert_eq!(p.to_string(), expected);
        // The spec's one-line example, and the head / atom signatures.
        let r = Rule::new(
            "p",
            vec![Term::var("a").into(), Term::str("X").into(), Term::int(3).into()],
            vec![
                Literal::pos(Atom::new("q", vec![Term::var("a")])),
                Literal::neg(Atom::new("r", vec![Term::var("a")])),
                Literal::cmp(CmpOp::Ne, Term::var("a"), Term::str("y")),
            ],
        );
        assert_eq!(r.to_string(), r#"p(a, "X", 3) :- q(a), !r(a), a != "y"."#);
        assert_eq!(r.head.signature(), "p/3");
        assert_eq!(r.head.arity(), 3);
        assert!(!r.is_fact());
        assert!(p.rules[0].is_fact());
        assert_eq!(Atom::new("edge", vec![Term::wild(); 3]).signature(), "edge/3");
    }

    #[test]
    fn append_remaps_sources() {
        let mut base = Program::new();
        assert_eq!(base.add_source("check"), 0);
        assert_eq!(base.add_source("overlay"), 1);
        let mut kept = Rule::new(
            "a",
            vec![Term::Var { name: "x".into(), span: sp(1, 2) }.into()],
            vec![Literal::Pos(Atom {
                pred: "b".into(),
                args: vec![Term::Var { name: "x".into(), span: sp(1, 7) }],
                span: sp(1, 5),
            })],
        );
        kept.span = sp(1, 0);
        kept.head.span = sp(1, 0);
        base.rules.push(kept.clone());

        let mut other = Program::new();
        assert_eq!(other.add_source("check"), 0);
        let mut moved = Rule::new(
            "c",
            vec![
                Term::Wild(sp(0, 2)).into(),
                HeadArg::Agg(Aggregate {
                    op: AggOp::Count,
                    vars: vec!["y".into()],
                    span: sp(0, 5),
                }),
            ],
            vec![
                Literal::Neg(Atom {
                    pred: "d".into(),
                    args: vec![Term::Str { value: "s".into(), span: sp(0, 12) }],
                    span: sp(0, 10),
                }),
                Literal::Cmp(Comparison {
                    op: CmpOp::Lt,
                    left: Term::Var { name: "y".into(), span: sp(0, 16) },
                    right: Term::Int { value: 4, span: sp(0, 20) },
                    span: sp(0, 16),
                }),
            ],
        );
        moved.span = sp(0, 0);
        moved.head.span = sp(0, 0);
        other.rules.push(moved.clone());

        base.append(other);
        assert_eq!(base.sources, vec!["check", "overlay", "check"]);
        assert_eq!(base.rules.len(), 2);
        assert_eq!(base.rules[0], kept);
        let after = sources_of(&base.rules[1]);
        assert_eq!(after.len(), 9);
        assert!(after.iter().all(|s| *s == 2), "{after:?}");
        assert_eq!(sources_of(&base.rules[0]), vec![1; 5]);
        // Only the source moved: offsets, lines and columns are untouched.
        assert_eq!(base.rules[1].body[1].span(), Span { source: 2, ..sp(0, 16) });
        assert_eq!(base.source_name(2), "check");
        assert_eq!(base.source_name(1), "overlay");
        assert_eq!(base.source_name(9), UNNAMED_SOURCE);
    }

    #[test]
    fn catalog_core_declares_the_core_set() {
        let carry = [EdgeCategoryId(1)];
        let c = Catalog::core(REG, &carry);
        let edb: Vec<String> = c.edb.iter().map(PredDecl::signature).collect();
        assert_eq!(
            edb,
            [
                "node/2",
                "edge/3",
                "edge_conf/4",
                "cell/3",
                "edge_cell/5",
                "kind/1",
                "category/1",
                "carry_category/1"
            ]
        );
        let builtins: Vec<String> = c.builtins.iter().map(BuiltinDecl::signature).collect();
        assert_eq!(
            builtins,
            [
                "reaches/2",
                "reaches/3",
                "reaches/4",
                "scc/2",
                "scc/3",
                "starts_with/2",
                "ends_with/2",
                "contains/2"
            ]
        );
        assert_eq!(c.registries, REG);
        assert_eq!(c.carry, carry);
        assert_eq!(
            c.edb("edge_conf").map(|p| p.cols.clone()),
            Some(vec![ColType::Node, ColType::Node, ColType::Category, ColType::Str])
        );
        let r4 = c.builtin("reaches", 4).cloned();
        assert_eq!(
            r4,
            Some(BuiltinDecl {
                name: "reaches".into(),
                cols: vec![ColType::Node, ColType::Node, ColType::CategorySet, ColType::Int],
                bind: vec![BindReq::May, BindReq::May, BindReq::Const, BindReq::Const],
                tiered: true,
                kind: BuiltinKind::Reaches,
            })
        );
        assert_eq!(c.builtin("scc", 3).map(|b| (b.kind, b.tiered)), Some((BuiltinKind::Scc, true)));
        let sw = c.builtin("starts_with", 2).cloned();
        assert_eq!(
            sw.map(|b| (b.bind, b.tiered, b.kind)),
            Some((vec![BindReq::Must, BindReq::Must], false, BuiltinKind::StartsWith))
        );
        assert!(c.builtin("reaches", 5).is_none());
        assert!(c.edb("reaches").is_none());
    }

    #[test]
    fn with_attrs_adds_name_2() {
        let c = Catalog::core(REG, &[]).with_attrs(&[
            AttrDecl { name: "qname", ty: ColType::Str },
            AttrDecl { name: "edge", ty: ColType::Int },
            AttrDecl { name: "line", ty: ColType::Int },
            AttrDecl { name: "qname", ty: ColType::Int },
            AttrDecl { name: "scc", ty: ColType::Str },
        ]);
        assert_eq!(
            c.edb("qname"),
            Some(&PredDecl { name: "qname".into(), cols: vec![ColType::Node, ColType::Str] })
        );
        // The core wins: `edge` keeps its three columns, the second `qname` and an
        // attribute named like a builtin are dropped.
        assert_eq!(c.edb("edge").map(|p| p.cols.len()), Some(3));
        assert!(c.edb("scc").is_none());
        let edb: Vec<String> = c.edb.iter().map(PredDecl::signature).collect();
        assert_eq!(&edb[8..], ["qname/2", "line/2"]);

        let in_scope = BuiltinDecl {
            name: "in_scope".into(),
            cols: vec![ColType::Node, ColType::Str],
            bind: vec![BindReq::May, BindReq::Must],
            tiered: false,
            kind: BuiltinKind::Domain,
        };
        let shadow =
            BuiltinDecl { name: "reaches".into(), kind: BuiltinKind::Domain, ..in_scope.clone() };
        let on_edb = BuiltinDecl { name: "qname".into(), ..in_scope.clone() };
        let c = c.with_builtins(vec![in_scope.clone(), shadow, on_edb, in_scope.clone()]);
        let builtins: Vec<String> = c.builtins.iter().map(BuiltinDecl::signature).collect();
        assert_eq!(&builtins[8..], ["in_scope/2"]);
        assert_eq!(c.builtin("reaches", 2).map(|b| b.kind), Some(BuiltinKind::Reaches));
        assert_eq!(c.builtin("in_scope", 2), Some(&in_scope));
    }
}
