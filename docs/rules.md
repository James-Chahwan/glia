# Rules and queries over the code graph

glia evaluates Datalog over the graph it builds: nodes, their kinds and locations, edges and their
categories, and cells. A query is answered as rows, each value located, each row tiered (FACT /
DERIVED / HEURISTIC), with the coverage caveats of any negation it went through. An empty answer
says why it is empty. The evaluator is the `glia-rules` crate; the code domain's schema and the
answer shape are the engine's (`glia_engine::rules`).

Entry points (Rust): `rules::query(merged, text, &QueryOptions)` answers a program,
`rules::query_gql(merged, text, &QueryOptions)` answers a GQL read query (below),
`rules::validate(text)` checks one against the code schema without a graph, and
`rules::evaluate_rule(merged, facts, source, text, "violation")` evaluates a rule invariant's
program for `glia check`. The command-line, Python and `glia check` surfaces of 0.5.2 call them,
so the language below is the same everywhere. A rule an agent proposes goes through
`glia overlay propose / try / accept`, which judges it on the code as it is (docs/overlay.md,
`Rules through the loop`).

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
- Two-character operators lex greedily, so `<-` is one token: write `a < -1`, never `a<-1`
  (and `{n: -1}`, never `{n:-1}`, in GQL).
- Identifiers are ASCII (letters, digits, `_`); a non-ASCII letter outside a string is an
  `unexpected character` error. A string constant may hold any UTF-8.
- A line ends at `\n` (a `\r\n` pair counts once); a lone `\r` is not a line break, so it
  does not end a `//` comment or move a diagnostic's line.

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
predicate and cannot be defined by a rule ``, whatever the arity), nor a prelude predicate or engine
relation below (`` `carry` is a prelude predicate (docs/rules.md); name yours differently ``,
whatever the arity), and names starting `__` are reserved for the evaluator's own rewriting.

## The prelude

Every program can read a set of named structural concepts, the **prelude**: rules shipped with
glia (`engine/src/rules/prelude.dl`, `glia_engine::rules::PRELUDE`) over the core predicates and
a few engine relations. A rule names the concept (`carry`, `depends`, `effect`, `unreached`) and
the prelude says which categories, kinds and tables that means, read from the code profile, never
listed by hand. When the profile changes (a carry edge added, a category split), the prelude's
answers change and your rules do not.

The prelude is merged into a program as it is read: only the prelude rules the program uses,
directly or through other prelude rules, join it (plus those of a prelude predicate asked for as an
output), so an unused one costs nothing, is never a default output, and adds no absence mechanism
or caveat. The names are a contract: renaming one would break the rules that use it.

| predicate | rule | holds when |
|---|---|---|
| `carry(A, B)` | `edge(A, B, C), carry_category(C)` | a semantic edge: one reachability, blast radius and liveness follow |
| `depends(A, B)` | `edge(A, B, C), dependency_category(C)` | a dependency edge: IMPORTS plus the carry edges, less TESTS and DOCUMENTS (`glia check`'s default forbid set) |
| `calls(A, B)` | `edge(A, B, "CALLS")` | a call |
| `uses(A, B)` | `edge(A, B, "USES")` | a use of a symbol (a type, a field, a function by value) |
| `imports(A, B)` | `edge(A, B, "IMPORTS")` | an import |
| `defines(P, C)` | `edge(P, C, "DEFINES")` | `P` defines `C` |
| `contains_node(P, C)` | `edge(P, C, "CONTAINS")` | `P` contains `C` (`contains/2` is the string test) |
| `implements(A, B)` | `edge(A, B, "IMPLEMENTS")` | `A` implements `B` |
| `inherits(A, B)` | `edge(A, B, "INHERITS_FROM")` | `A` inherits from `B` |
| `handles(R, H)` | `edge(R, H, "HANDLED_BY")` | route `R` is handled by `H` |
| `handled(R)` | `handles(R, _)` | route `R` has a handler |
| `unhandled_route(R)` | `node(R, "ROUTE"), !handled(R)` | a route with no handler in the graph (caveated by HANDLED_BY) |
| `tested(N)` | `edge(_, N, "TESTS")` | `N` has a test |
| `unreached(N)` | `node(N, K), symbol_kind(K), !live(N)` | a CLASS, FUNCTION or METHOD no entry point reaches over the carry edges (caveated by every carry category) |
| `effect(F, S, Class)` | `edge(F, S, C), node(S, K), effect_sink(K, C, Class)` | `F` reaches effect sink `S` directly, classed by the profile (`db`, `http_call`, `queue_produce`, ...) |
| `cross_repo(A, B, C)` | `edge(A, B, C), repo(A, R1), repo(B, R2), R1 != R2` | an edge of category `C` whose ends sit in two repositories of a merge |
| `module_dep(A, B)` | `module_import(A, B)` | MODULE `A` imports MODULE `B` |

`unreached` is liveness alone, not `glia gaps`' `dead_symbol`: it keeps no dispatch or
test-fixture exclusion, and a symbol it lists may still be called (its caveats say how). `effect`
is one hop, the edge into the sink; `glia effects` walks the carry edges to the sinks a function
reaches.

**Engine relations.** Each is computed by the engine only when a rule the program uses reads it, and
every column may be bound or free:

| relation | rows | tier | a negation of it reads |
|---|---|---|---|
| `dependency_category(C)` | `glia check`'s default forbid categories | FACT | nothing |
| `symbol_kind(K)` | `CLASS`, `FUNCTION`, `METHOD` (the kinds of `glia gaps`' `dead_symbol`) | FACT | nothing |
| `effect_sink(K, C, Class)` | every (kind, category) pair the profile's effect sinks name, with the class the first matching sink gives it | FACT | nothing |
| `live(N)` | the nodes liveness reaches from the entry points (`entrypoint_reachable`) | DERIVED (a computed walk) | every carry category |
| `module_import(A, B)` | the import graph lifted to MODULE nodes (`glia cycles`' import graph): one row per module pair | the tier `glia why` gives the import behind it | IMPORTS |

Every evaluation prints `[rules] prelude relations=<n> (dependency_category=<0|1> symbol_kind=<0|1>
effect_sink=<0|1> live=<0|1> module_import=<0|1>)` on stderr: the relations it built.

The prelude reads structure only: no prelude predicate or engine relation names authentication,
authorization, secrets or any other security property, none reads a value or data flow, and none
reads source text. A test enforces it (SECURITY.md).

## Outputs

The predicates a query answers are the ones asked for (`QueryOptions::outputs`, as `name` or
`name/arity`; a prelude predicate may be asked for), in the order asked. With none asked, they are
the program's **sinks**: every predicate it defines that no rule of another predicate reads, in
first-definition order (a prelude predicate never is one). So in

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
`reaches` / `scc` category set (the carry set when it names none), an engine relation's own
categories (`live` reads every carry category, `module_import` IMPORTS), through every predicate the
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
no value- or data-flow relation to query, the prelude defines no authentication, authorization or
security predicate, and glia ships no rule that classifies routes by authentication (SECURITY.md,
`The query language reads structure only`).

## GQL read subset

A GQL read query (MATCH ... WHERE ... RETURN) is compiled to a rule program whose output is
`result`, so it shares the Datalog evaluator, the tier lattice, the caveats and the row budget.
It is a read *subset* of GQL (ISO/IEC 39075) and openCypher, not a conforming implementation: the
deviations below are deliberate. The engine entry point is
`rules::query_gql(merged, text, &QueryOptions)`; it returns the same answer shape as a Datalog
query, with `language: "gql"`.

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

**Lowering.** Every pattern variable is a rule variable of the same name; an anonymous node, an
attribute value and an unrolled intermediate get a fresh `_g<n>`, and helper predicates are named
`gql_<what><n>`.

| construct | lowered to |
|---|---|
| `(a:FUNCTION)`, `(a:CLASS\|METHOD)` | `node(a, "FUNCTION")`; one alternative per label. A node no atom binds gets `node(a, _)` |
| `(a {qname: 'x'})`, `a.qname` | the attribute atom `qname(a, "x")`, `qname(a, _g0)` |
| `(a)-[:CALLS]->(b)` | `edge(a, b, "CALLS")`; `<-` swaps the ends, `-[]-` is two alternatives, no type is `edge(a, b, _)` |
| `-[:CALLS\|USES]->` | `gql_hop0(a, b)`, one rule per type: `gql_hop0(x, y) :- edge(x, y, "CALLS").` |
| `-[r]->`, `type(r)` | `edge(a, b, r)`: `r` is the category |
| `-[:CALLS*]->`, `+`, `{1,}` | `reaches(a, b, "CALLS")` (`"*"` when untyped): reachability, never unrolled |
| `{0,}`; `{m,}` | `reaches` plus the alternative `a = b`; m - 1 hops, then `reaches` |
| `*m..n`, `{m,n}` (n at most 8) | one alternative per length, unrolled: `{2,3}` is `edge(a, _g0, "CALLS"), edge(_g0, b, "CALLS")` and a three-hop body |
| `WHERE` AND / OR / NOT | negation normal form (NOT flips a comparison), then one rule body per disjunct |
| `=`, `<>`, `<`, `<=`, `>`, `>=` | a comparison |
| `STARTS WITH`, `ENDS WITH`, `CONTAINS` | `starts_with`, `ends_with`, `contains` |
| `IN ['x', 'y']` | a fact table: `gql_in0("x"). gql_in0("y").` |
| `NOT (pattern)`, `NOT EXISTS {pattern}` | a helper over the pattern, negated: `!gql_not0(f)`, `gql_not0(f) :- edge(_, f, "CALLS").` |
| a pattern or `EXISTS {..}` in `WHERE` | inlined; its own variables are local |
| `in_scope(a, 'web')`, `live(f)` | the builtin of that name: `in_scope`, `under`, the string tests, an engine relation (`live`, `symbol_kind`, `effect_sink`, `dependency_category`, `module_import`) |
| `RETURN a, b.qname` | the head `result(a, _g0)`, with `qname(b, _g0)` in the body |
| `count(n)`, `min`, `max`, `sum` | a grouped head `result(_g0, count(n))`; over several alternatives through one union helper `gql_match0` |
| `count(*)` | `count` over every MATCH node and relationship variable |
| `p = [ANY SHORTEST] (a)-[..*]->(b)` | the head holds `a` and `b`; the engine renders the row's witness as `p` |
| `ORDER BY`, `LIMIT` | applied by the engine to the rendered rows |

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

**Answers.** One output, `result`, whose columns are the RETURN items' aliases, else their text
(`a.qname`, `count(*)`). A property is a string or integer value; a node variable is a located
node, as in Datalog.

- **Order.** Rows come in rendered order, then are sorted stably by the ORDER BY keys' rendered
  values: qnames, strings and integers, never node ids, so the order is the same on every
  checkout. `DESC` flips a key, and the full rendered row breaks ties.
- **LIMIT** cuts the sorted rows, then the caller's limit (`QueryOptions::limit`) cuts them again;
  `total` keeps the uncut count. `SKIP` is refused.
- **A path column** (`RETURN p`) is the row's witness rendered as the walk from the path's start
  to its end, qnames joined by hops: `a -CALLS-> b -CALLS-> c`, and `<-CALLS-` for a hop walked
  against its edge (`p = (c)<-[:CALLS*]-(a)` renders `c <-CALLS- b <-CALLS- a`). The walk is the
  first run of the witness's edge leaves that joins the two ends, so an edge another pattern of
  the query binds is not mistaken for a hop. A zero-hop path (`{0,}`) renders as its one node,
  and a walk the witness's leaf cap cut renders as `a ... c`. The witness is the shortest path at
  the row's tier, ties broken by qname (`## Witnesses`), so a path row is checkout-stable. The
  row is located at the path's start node and carries its witness rows with or without
  `explain`. Two paths with different end nodes are two rows.

**Errors.** A GQL query's diagnostics name `<gql>`, or the caller's source name
(`QueryOptions::source_name`) when it names one other than `<query>`. A parse error and every
refusal below are kind `parse`; an unknown label, relationship type or property is kind
`analysis`, located at the label (`unknown node kind `ROUTEE`; did you mean `ROUTE`?`).

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

### Worked examples

Each is answered on the three-project fixture of `## A first query` (`web/app.py` `page()` calls
`shared.util.helper` and `other()` calls `services.api.public.facade.get`; `helper()` calls
`services.api.internal.db.query`; `get()` calls `db.write`; one overlay `[[edge]]` adds
`web::app::page -> services::api::internal::db::write` CALLS). engine/tests/rules_gql.rs pins
every answer here.

**1. Match, filter, order.**

```text
MATCH (a:FUNCTION)-[:CALLS]->(b:FUNCTION) WHERE a.qname STARTS WITH 'web::'
RETURN a.qname, b.qname ORDER BY a.qname
```

| a.qname | b.qname | tier |
|---|---|---|
| `web::app::other` | `services::api::public::facade::get` | fact |
| `web::app::page` | `services::api::internal::db::write` | heuristic |
| `web::app::page` | `shared::util::helper` | fact |

`ORDER BY a.qname` ties on `web::app::page`; the full rendered row breaks the tie.

**2. The same query in Datalog.** The query above lowers to

```text
result(_g0, _g1) :- node(a, "FUNCTION"), edge(a, b, "CALLS"), node(b, "FUNCTION"),
                    qname(a, _g0), starts_with(_g0, "web::"), qname(b, _g1).
```

and this hand-written program gives the same rows with the same tiers, from the same evaluator:

```text
result(x, y) :- edge(a, b, "CALLS"), node(a, "FUNCTION"), node(b, "FUNCTION"),
                qname(a, x), qname(b, y), starts_with(x, "web::").
```

**3. Reachability across scopes.**

```text
MATCH (a)-[:CALLS*]->(b) WHERE in_scope(a, 'web') AND in_scope(b, 'services/api')
RETURN DISTINCT b.qname ORDER BY b.qname
```

| b.qname | tier |
|---|---|
| `services::api::internal::db::query` | fact |
| `services::api::internal::db::write` | fact |
| `services::api::public::facade::get` | fact |

`db::write` is FACT although one way to it is the overlay edge: `other -> get -> write` is parsed,
and a `reaches` row takes its best path's tier.

**4. Aggregate and limit.**

```text
MATCH (m:MODULE)-[:IMPORTS]->(n) RETURN m.qname AS module, count(n) AS imports
ORDER BY imports DESC, module LIMIT 2
```

| module | imports | tier |
|---|---|---|
| `web::app` | 2 | fact |
| `services::api::public::facade` | 1 | fact |

`total` is 3: `shared::util` also imports one name, and LIMIT cut it.

**5. A negated pattern.**

```text
MATCH (f:FUNCTION) WHERE NOT ()-[:CALLS]->(f) RETURN f.qname
```

| f.qname | tier |
|---|---|
| `web::app::other` | fact |
| `web::app::page` | fact |

Both rows carry the `CALLS` coverage caveats (`## Negation and coverage caveats`): "nothing calls
it" holds in the graph, and the caveats say where extraction is blind.

**6. A path.**

```text
MATCH p = ANY SHORTEST (a {qname: 'web::app::page'})-[:CALLS*]->
          (b {qname: 'services::api::internal::db::query'})
RETURN p
```

| p | tier | at |
|---|---|---|
| `web::app::page -CALLS-> shared::util::helper -CALLS-> services::api::internal::db::query` | fact | web/app.py:5 |

Its witness rows are the two calls, each at its call site. The overlay edge from `page` leads to
`db::write`, not to `db::query`, so it is in no path here.
