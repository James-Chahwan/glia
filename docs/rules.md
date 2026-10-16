# Rules and queries over the code graph

glia evaluates Datalog over the graph it builds: nodes, their kinds and locations, edges and their
categories, and cells. A query is answered as rows, each value located, each row tiered (FACT /
DERIVED / HEURISTIC), with the coverage caveats of any negation it went through. An empty answer
says why it is empty. The evaluator is the `glia-rules` crate; the code domain's schema and the
answer shape are the engine's (`glia_engine::rules`).

Entry points (Rust): `rules::query(merged, text, &QueryOptions)` answers a program,
`rules::validate(text)` checks one against the code schema without a graph, and
`rules::evaluate_rule(merged, facts, source, text, "violation")` evaluates a rule invariant's
program for `glia check`. The command-line, Python and `glia check` surfaces of 0.5.2 call them,
so the language below is the same everywhere.

## A first query

```text
x(a, b) :- edge(a, b, "CALLS"), in_scope(a, "web").
```

Every `CALLS` edge whose caller sits under `web/` (a path, or a project label: `in_scope` resolves
it as every `--scope` flag does). On a three-project fixture (`web/`, `shared/`, `services/api/`)
with one overlay `[[edge]]` the answer is:

| a | b | tier | at |
|---|---|---|---|
| `web::app::other` | `services::api::public::facade::get` | fact | web/app.py:9 |
| `web::app::page` | `services::api::internal::db::write` | heuristic | web/app.py:5 |
| `web::app::page` | `shared::util::helper` | fact | web/app.py:5 |

The second row's edge comes from the overlay, so it is HEURISTIC; the parsed calls are FACT.

## Programs

```text
program  := clause*
clause   := head ( ':-' body )? '.'
head     := ident ( '(' [ harg (',' harg)* ] ')' )?
harg     := aggop '(' ident (',' ident)* ')' | term      aggop = count | min | max | sum
body     := literal (',' literal)*
literal  := ('!' | 'not') atom | atom | term cmpop term
atom     := ident '(' [ term (',' term)* ] ')'
term     := ident | '_' | string | int | '-' int
cmpop    := '=' | '!=' | '<>' | '<' | '<=' | '>' | '>='
```

- A bare identifier in an argument is a **variable**, whatever its case (`Route` is a variable
  too); a **constant** is a quoted string (`"ROUTE"` or `'ROUTE'`, escapes `\\ \" \' \n \t`) or a
  decimal integer. `_` is the wildcard: some value, never bound, never in a head or a fact.
- A clause without a body is a **fact**: `scope("web").`
- `!p(x)` and `not p(x)` negate an atom. Every variable of a negated atom must be bound by a
  positive atom of the same rule, and negation may not sit inside recursion (the program must
  stratify).
- A comparison needs both sides bound, except `=` with one free variable side, which assigns it.
  `<`, `<=`, `>`, `>=` compare integers numerically and strings by bytes.
- An aggregate is a head argument only: `fan(y, count(x)) :- edge(x, y, "CALLS").` groups by the
  head's other terms. A predicate with an aggregate head has exactly one rule and is not recursive.
  Rows are sets, so `count` counts distinct values.
- Comments run `//` to the end of the line, or `/* ... */` (not nested).
- A 0-arity atom takes parentheses in a body (`done()`); a 0-arity head may be bare (`ok.`).
- Two-character operators lex greedily, so `<-` is one token: write `a < -1`, never `a<-1`.

## The schema

Names of node kinds, edge categories and cell types are the code domain's registry names
(`FUNCTION`, `ROUTE`, `CALLS`, `HANDLED_BY`, `POSITION`, ...; the registries are in
`code-domain/src/lib.rs`, and `glia inspect <layout>` names the ones a built graph holds). A
constant in a kind / category / cell-type column that is not a registered name is an analysis
error with the nearest name suggested.

### Core predicates

| predicate | columns | rows |
|---|---|---|
| `node(N, Kind)` | node, kind name | every node, once (an id two language graphs share is one node) |
| `edge(A, B, Category)` | node, node, category name | every edge, as a set: two call sites of one callee are one row |
| `edge_conf(A, B, Category, Conf)` | + `strong` / `medium` / `weak` | the edge's confidence |
| `cell(N, CellType, Text)` | node, cell-type name, text | every node cell |
| `edge_cell(A, B, Category, CellType, Text)` | | every edge cell |
| `kind(Kind)`, `category(Category)` | | the registries |
| `carry_category(Category)` | | the categories reachability, blast radius and liveness follow (the code profile's carry edges) |

### Code attributes

Each is the predicate `name(Node, Value)`, read only when a program names it.

| attribute | value |
|---|---|
| `qname(N, Q)` | the qualified name (`web::app::page`) |
| `name(N, S)` | the short name |
| `file(N, F)` | the repo-relative file the node is located in; an unlocated node has no row |
| `line(N, L)` | the 1-based line (an integer) |
| `repo(N, R)` | `repo#<k>`, the node's repository by its position in the merge, so it is the same on every checkout |
| `project(N, P)` | the label of the PROJECT the node's file sits under (the deepest); a PROJECT's own label |
| `parent(N, P)` | the enclosing node |
| `role(N, R)` | a role kind name the node plays (`graph::roles`) |
| `origin(N, O)` | an ORIGIN cell's provenance: `external`, `test_fixture`, `nav_route`, ... |

### Builtins

| builtin | arguments bound before the call | holds when |
|---|---|---|
| `in_scope(N, Scope)` | `Scope` | `N` sits in the scope: a path (segment boundary: `web` holds `web/app.py`, not `webhooks/`), a project label or a `project:<path>` qname. A node is in a scope by the file it is located in; a PROJECT by its path |
| `under(Path, Prefix)` | both | the path string sits under the prefix on a segment boundary |
| `reaches(X, Y)` | none | `Y` is reachable from `X` in one or more hops over the carry categories |
| `reaches(X, Y, "A\|B")` | the category set, a literal | over the named categories; `"*"` is every category |
| `reaches(X, Y, "A\|B", D)` | the set and the depth, literals | in 1 to `D` hops |
| `scc(X, Rep)`, `scc(X, Rep, "A\|B")` | the set, a literal | `X` is in a strongly connected component that holds a cycle, whose representative `Rep` is its smallest member by qname |
| `starts_with(S, P)`, `ends_with(S, P)`, `contains(S, P)` | both | string tests |

`reaches(x, x)` holds exactly when `x` is on a cycle. Closures run as one breadth-first walk per
category set, never as generic recursion, so `reaches` is the way to ask for reachability.

### Reserved names

A rule may not define a core predicate, a code attribute or a builtin (`` `edge/2` is a built-in
predicate and cannot be defined by a rule ``, whatever the arity), and names starting `__` are
reserved for the evaluator's own rewriting.

## Outputs

The predicates a query answers are the ones asked for (`QueryOptions::outputs`, as `name` or
`name/arity`), in the order asked. With none asked, they are the program's **sinks**: every
predicate it defines that no rule of another predicate reads, in first-definition order. So in

```text
uncalled(f) :- node(f, "FUNCTION"), !called(f).
called(f) :- edge(_, f, "CALLS").
```

the answer is `uncalled/1` alone: `called` is a helper. Keeping helpers out of the outputs also
lets the evaluator seed a recursive helper from its bound callers (magic sets): an output is never
rewritten, so it is computed whole. Asking for a predicate no rule defines is an analysis error.

## Answers

Each output reports its predicate, arity, columns (the head variables of its first rule, in source
order; a constant position is `c<i>`, 0-based; an aggregate `count(x)`), `total` (its rows) and
`shown` (after `limit`). Rows follow, output by output.

- A **node value** is its qname, kind and 1-based `file:line`, all from one locator over the graph;
  a string or integer is itself. JSON: `{"node": {"qname", "kind", "file", "line"}}`,
  `{"str": ".."}`, `{"int": n}`.
- A row's `file` / `line` are its first node value's.
- **Order.** Rows sort by their rendered values column by column: integers before strings before
  nodes; a node by qname, then file, then line. Node ids never order anything: an id hashes a
  path-derived repository id and differs between two checkouts of one commit, while the rendered
  order does not.
- `limit` cuts each output after the sort; `total` keeps the full count.

## Tiers

Every row carries the evidence tier of its weakest input, minimised over its derivations: the
**min over derivations of the max over inputs**, evaluated exactly (each stratum runs to fixpoint
with only FACT inputs visible, then adds DERIVED, then HEURISTIC, and a row is kept at the first
level that derives it).

- An edge's tier is the one `glia why` gives it: FACT when a parser read it at a site, DERIVED when
  a resolver or pass paired it (or a graph binding inferred it), HEURISTIC for a name-only binding,
  an overlay edge, git history or a model.
- Node, attribute, cell and registry rows are FACT; `in_scope`, `under` and the string tests are
  untiered (they read structure only).
- `reaches` and `scc` rows take the tier of their best path: the first of the FACT, DERIVED and
  HEURISTIC walks that reaches them.

## Negation and coverage caveats

A negated atom reads an absence, and "no edge" in the graph is not "no call" in the code: glia's
extraction has known blind spots (`glia coverage`). So negation adds no tier, but a row derived
through a negation carries the **coverage caveats** of every edge category the negated sub-program
reads: the caveat rows of `glia coverage` for those categories, for the languages in the graph.
The categories are found statically and conservatively: an `edge` atom's constant category, a
`reaches` / `scc` category set (the carry set when it names none), through every predicate the
negated one reads. A category read through a variable, `_` or `"*"` counts as every category.
Positive atoms only ever tier a row.

The `uncalled` rows above carry the `CALLS` caveats (reflection, dynamic dispatch and higher-order
calls are not resolved): a function listed may still be called.

## Witnesses

With `explain`, each shown row carries its witness: the leaf edges of its minimal-height
derivation at its tier, depth-first in body order, each located at its evidence site (the call
site, the overlay stanza, ...) with the tier and note `glia why` gives that edge, at most 100. A
`reaches` row's witness is a shortest path at its tier, ties broken by qname, so a witness is the
same on every checkout.

## Empty answers

When no output has a row, the answer carries an absence: a FACT about the graph as built,
`reason: no_match`, the note `no row of x/1 in this graph` (each output named), the edge categories
the outputs' rules read (positive or negated), and those categories' coverage caveats.

## Errors

An error is one of four kinds, each with diagnostics located at `<source>:<line>:<column>`
(1-based, within the program text) and naming the rule (`in the rule for `x/1``):

| kind | examples |
|---|---|
| `parse` | `expected `.` after the rule for `v/1`, found `w``, `unterminated string` (at most 20 per program, one per broken clause) |
| `analysis` | `unknown node kind `ROUTEE`; did you mean `ROUTE`?`, `unknown edge category `CALL`; did you mean `CALLS`?`, `unknown predicate `nope/1``, `predicate `p` is used with 2 arguments here but defined with 1`, `variable `y` in the head of `v/1` is not bound by the body`, `negation inside recursion: `p/0`, `q/0` depend on each other through this negated atom`, `unknown output `missing`; the program defines x/1` |
| `budget` | the row budget below |
| `evaluation` | a builtin failure or a run-time type error (`sum over a non-integer value`) |

Warnings do not stop a query; they print on stderr as `<source>:<line>:<col>: warning: ...`:
`variable `y` appears once; write `_``, and the unseeded-recursion warning below.

## The row budget

An evaluation stops when it has derived more rows than its budget (5,000,000 by default; the
engine's `QueryOptions::row_budget`), and never truncates silently: the answer is an error naming
the predicate and its first rule.

```text
rule evaluation stopped: `r/2` (<query>:1) reached 101 derived rows against a row budget of 100.
Its recursion is probably unseeded: bind an argument from a small relation (`r(x, y) :- seed(x), ...`),
use reaches(x, y, "CATS") for reachability, or raise the budget.
```

The usual cause is an **unseeded recursion**: `r(a, c) :- r(a, b), edge(b, c, "CALLS")` whose base
rule starts from every edge computes the whole closure, quadratic in a large graph. Analysis warns
about it (`recursive predicate `r/2` is unseeded: its recursion starts from every row of
`edge/3`; ...`). Seed it from a small relation, read it from a bound caller (the evaluator then
rewrites it to the demanded rows), or use `reaches`, which walks the graph once per category set.

## Determinism

One program over one graph gives one answer, byte for byte: rows sort by rendered values, witness
choices compare rendered keys, and no hash map is iterated. Two checkouts of one commit at two
paths answer alike.

## What the evaluator reads

Structure only: nodes, their kinds and locations, edges and their categories, and cells. There is
no value- or data-flow relation to query, and glia ships no rule that classifies routes by
authentication (SECURITY.md).

## GQL read subset

A GQL read query (MATCH ... WHERE ... RETURN) is compiled to a rule program whose output is
`result`, so it shares the Datalog evaluator, the tier lattice, the caveats and the row budget.

```text
query    := match+ [WHERE expr] RETURN [DISTINCT] item (',' item)* [ORDER BY key (',' key)*] [LIMIT int] [';']
match    := MATCH pattern (',' pattern)*
pattern  := [ident '=' [ANY SHORTEST]] node (rel node)*
node     := '(' [ident] [':' ident ('|' ident)*] ['{' ident ':' lit (',' ident ':' lit)* '}'] ')'
rel      := '-[' body ']->' [gquant] | '<-[' body ']-' [gquant] | '-[' body ']-' [gquant] | '-->' | '<--' | '--'
body     := [ident] [':' ident ('|' ident)*] ['*' [int] ['..' [int]]]
gquant   := '+' | '*' | '{' int ',' [int] '}' | '{' int '}'
expr     := OR < AND < NOT < cmp
cmp      := operand (('=' | '<>' | '!=' | '<' | '<=' | '>' | '>=') operand | STARTS WITH operand
            | ENDS WITH operand | CONTAINS operand | IN '[' lit (',' lit)* ']')
            | pattern | EXISTS '{' pattern '}' | ident '(' [operand (',' operand)*] ')' | '(' expr ')'
operand  := ident | ident '.' ident | lit | TYPE '(' ident ')'
item     := (operand | (COUNT | MIN | MAX | SUM) '(' ('*' | [DISTINCT] operand) ')') [AS ident]
key      := (ident | ident '.' ident) [ASC | DESC]
lit      := string | ['-'] int
```

A label is a node kind (`(f:FUNCTION)`), a relationship type an edge category (`-[:CALLS]->`), a
property a code attribute (`a.qname`, `{qname: 'web::app::page'}`), and a function call a builtin
(`in_scope(a, 'web')`). Keywords compare case-insensitively.

**Semantics.**

- Rows are DISTINCT (`RETURN DISTINCT` changes nothing), and `count(*)` counts the distinct
  bindings of every MATCH node and relationship variable.
- Two relationship patterns may bind the same edge (edge homomorphism, unlike openCypher).
- No nulls: a node without an attribute the query reads drops the row, under NOT too
  (`NOT a.k = 'x'` keeps only nodes whose `k` is not `x`).
- `*`, `+` and `{1,}` are reachability (`reaches`); `{0,}` adds the zero-hop pair; `*m..n` and
  `{m,n}` are walk lengths, one alternative per length.
- Variables of a WHERE pattern that MATCH does not bind are local to it (existential).
- A bare path variable (`p = (a)-[:CALLS*]->(b)`) is the same as `ANY SHORTEST`: glia returns
  endpoint pairs, each with one shortest witness.

**Refused**, each by name at its keyword, with a message citing this page: `OPTIONAL MATCH`,
`WITH`, `UNWIND`, `UNION`, `CALL`, `CREATE`, `MERGE`, `SET`, `DELETE`, `DETACH DELETE`, `REMOVE`,
`FOREACH`, `LOAD`, `SKIP`, `CASE`, `$` parameters, `shortestPath()` / `allShortestPaths()`,
`ALL SHORTEST`, and the path restrictors `TRAIL` / `ACYCLIC` / `SIMPLE`. Also refused, each a
located diagnostic:

- a bounded repetition above 8 hops (`use * (any length)`), and an unbounded one whose lower bound
  is above 9 hops;
- an undirected variable-length relationship (reachability follows edges one way);
- boolean literals (`TRUE` / `FALSE`: rule values are nodes, strings and integers);
- a relationship or path variable naming more than one pattern;
- `result` or a `gql_` name as a property key or function name (the compiled program's own);
- a query that expands to more than 64 alternatives.

**Reserved words** (never a variable or alias; a label, type or property key may spell one):
`MATCH`, `WHERE`, `RETURN`, `ORDER`, `LIMIT`, the refused clause words (`OPTIONAL`, `WITH`,
`UNWIND`, `UNION`, `CALL`, `CREATE`, `MERGE`, `SET`, `DELETE`, `DETACH`, `REMOVE`, `FOREACH`,
`LOAD`, `SKIP`), and `AND`, `OR`, `NOT`, `XOR`, `AS`, `DISTINCT`, `IN`, `STARTS`, `ENDS`,
`CONTAINS`, `EXISTS`, `BY`, `ASC`, `DESC`, `TRUE`, `FALSE`, `CASE`.

**Spacing.** The lexer reads `:-` and `<-` as one token each, so a negative literal after `:` or
`<` needs a space: `{n: -1}`, never `{n:-1}`; `a < -1`, never `a<-1`.
