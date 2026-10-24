# Edge-audit rubric

The judge reads **source**, never the graph's own explanation: `show` prints the evidence site
(+-4 lines, the site line marked `>`) and the target's definition (+-2 lines); open more of the
file when that is not enough. The emitter, rule and tier note say which mechanism wrote the
edge so the judge knows what claim to check; they are not evidence that it is true.

## Verdicts

- `correct`: the source makes the edge's claim true, by the category's rule below.
- `wrong`: the source contradicts it: the target is not the thing the site refers to, the site
  does not do what the category says, or the edge's direction is reversed.
- `unsure`: the source cannot settle it (generated code, dynamic dispatch past what the files
  show, a target outside the snapshot). Say why in `reason`; unsure rows are excluded from p.

One verdict row per sample: `{"id": "<16 hex>", "verdict": "correct|wrong|unsure", "reason":
"<one line>", "judge": "<name>"}`.

## Per category

- **DEFINES / CONTAINS / HAS_ATTRIBUTE** (structural): the from node's source span declares or
  holds the target (a module defines the function, a class holds the method or field).
- **CALLS**: the site invokes the target, or one of the target's dispatch targets (the
  interface method a call on an interface value names, an override reached by inheritance).
  A same-named function of another module or type is wrong.
- **USES**: the site names the target symbol (a type in a signature, a value passed by name, a
  field or constant read).
- **IMPORTS**: the importer's import statement resolves to the target module under the
  language's resolution rules (relative path, package name, re-export barrel).
- **INHERITS_FROM / IMPLEMENTS**: the from type declares the target as a base / interface, or
  (Go, implicit) its method set satisfies the target interface; a method-level IMPLEMENTS pairs
  the member with the member it overrides.
- **RETURNS_TYPE / INJECTS**: the from callable's declared return type, or the injected
  parameter / field type, is the target.
- **HANDLED_BY**: the target is the code that runs for the route / topic / procedure.
- **HTTP_CALLS / GRPC_CALLS / RPC_CALLS / GRAPHQL_CALLS / WS_CONNECTS / QUEUE_FLOWS /
  EVENT_FLOWS / CLI_INVOKES**: the two ends name the same channel (method + path, service +
  method, subject, event, command) and the client could reach that server (the base URL, host
  or broker does not rule it out).
- **ACCESSES_DATA / READS_CONFIG / DEFINES_CONFIG / SCHEDULES / INFRA_REFERENCES /
  DEPENDS_ON**: the site really reads / writes the data entity, reads or defines the config key,
  registers the schedule, references the infrastructure resource, or declares the dependency.
  A string in a test literal or a comment that the code never executes is wrong.
- **DOCUMENTS**: the document's text is about the target (names it as its subject), not a
  passing mention of a same-named thing.
- **TESTS**: the from test exercises the target (calls it, or the module under test is it).
- **SAME_AS**: both nodes are the same thing by the rule the evidence names (one package, one
  message fqname, one contract operation, one model).
- **SHARES_*** / **CO_CHANGES** / **NAVIGATES_TO**: the shared key (schema, entity, config,
  schedule, dependency) is really the same on both sides; the navigation target is the page the
  site navigates to.
