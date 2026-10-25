"""The Sibyl templates, one module per transport. gen.py renders a case through
`TRANSPORTS[case["axes"]["transport"]]`, whose module provides `render(case, state)`,
`expected(case)` and `STATES`; a module may also hold `UNEXPRESSIBLE`, rows of
`((axis, value), (axis, value), why)` that gen.py adds to its own table, so a transport's
templates can state what its stacks cannot express."""

from . import rest

TRANSPORTS = {"rest": rest}
