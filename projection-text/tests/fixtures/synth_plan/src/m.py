"""Synth-plan fixture: the research synth hooks (LD.12b, LD.12d, LD.12e) run
over this one module. Inner/Outer and Target/Source are the class sources of
composition.rs's one_hop_typed_return_path and
docstring_backtick_fallback_enables_path tests; ListField/TupleField both
define `bind(self, schema)`, called from two distinct callers, so the
callsite-argflow hook sees one polymorphic name."""


class Inner:
    def __init__(self):
        self.value = 1


class Outer:
    def __init__(self):
        self.inner = Inner()

    @property
    def get_inner(self) -> Inner:
        return self.inner

    def use(self):
        return self.get_inner.value


class Target:
    def __init__(self):
        self.opts = None


class Source:
    def __init__(self):
        self._t = Target()

    @property
    def resolve(self):
        """Reference to the `Target` this belongs to."""
        return self._t

    def use(self):
        return self.resolve.opts


class ListField:
    def bind(self, schema):
        self.schema = schema


class TupleField:
    def bind(self, schema):
        self.schema = schema


class Schema:
    def attach(self, field):
        field.bind(self)


class Nested:
    def attach(self, inner):
        inner.bind(self)
