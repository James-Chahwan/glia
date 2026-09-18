"""File watcher: a debouncer's trigger() is a method call, not an event."""


class Debouncer:
    def trigger(self):
        pass


def start_watcher(debouncer: Debouncer) -> None:
    debouncer.trigger()
