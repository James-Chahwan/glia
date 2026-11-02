class SchemaMeta(type):
    def __new__(mcs, name, bases, attrs):
        klass = super().__new__(mcs, name, bases, attrs)
        klass.opts = {"ordered": attrs.get("ordered", False)}
        return klass


class Schema(metaclass=SchemaMeta):
    def dump(self, obj):
        if self.opts["ordered"]:
            return sorted(obj.items())
        return dict(obj)
