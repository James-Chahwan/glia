class Scheduler:
    def __init__(self):
        self.hooks = []

    def before_request(self, fn):
        self.hooks.append(fn)


app = Scheduler()


def tick():
    return 1


app.before_request(tick)
