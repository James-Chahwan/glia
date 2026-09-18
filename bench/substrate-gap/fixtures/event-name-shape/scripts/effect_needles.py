"""Classifier needle table: the strings are data, not event names."""

EVENT_NEEDLES = [
    "@OnEvent(", "@EventPattern(",
    "Subject.next(",
]


def is_event_call(line: str) -> bool:
    return any(n in line for n in EVENT_NEEDLES)
