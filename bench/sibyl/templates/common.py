"""Shared pieces of the Sibyl templates: the token filler, the exchanged field's names, the
route the REST exchange uses."""

import re
import textwrap
from dataclasses import dataclass

TOKEN = re.compile(r"<<([a-z_]+)>>")

# The one REST exchange every case renders: GET a user by id.
PREFIX = "/api/v1"
RESOURCE = "users"
PATH = PREFIX + "/" + RESOURCE + "/{id}"
HOST = "http://localhost:8080"


def fill(text, **kw):
    """Dedent `text`, then replace each `<<token>>` with its keyword value; a token with no
    value is an error (a template typo must fail loudly, never render)."""

    def sub(m):
        k = m.group(1)
        if k not in kw:
            raise KeyError(f"template token <<{k}>> has no value")
        return str(kw[k])

    return TOKEN.sub(sub, textwrap.dedent(text).lstrip("\n"))


def join(*parts):
    """The parts that are not None, one per line ("" is a blank line), ending in a newline."""
    return "\n".join(p for p in parts if p is not None) + "\n"


@dataclass(frozen=True)
class Name:
    """One JSON field name and its per-language spellings."""

    camel: str

    @property
    def snake(self):
        return re.sub(r"(?<!^)(?=[A-Z])", "_", self.camel).lower()

    @property
    def pascal(self):
        return self.camel[:1].upper() + self.camel[1:]

    def spellings(self):
        return {self.camel, self.snake, self.pascal}


# The rename drift: the server renames this response field at S, the client follows at C.
OLD = Name("displayName")
NEW = Name("fullName")


def server_field(state):
    return OLD if state == "P" else NEW


def client_field(state):
    return NEW if state == "C" else OLD


PARAM_SEGMENT = re.compile(r"\{[^}]*\}|:\w+|<[^>]*>|\$\{[^}]*\}|\$\w+|\\\([^)]*\)")


def norm_path(path):
    """A route or endpoint path with every parameter segment spelled `{}`: `{id}`, `:id`,
    `<int:id>`, `${...}` (glia's `${…}` included), `$id` and Swift's `\\(id)` all compare equal."""
    return "/".join("{}" if PARAM_SEGMENT.fullmatch(s) else s for s in path.split("/"))
