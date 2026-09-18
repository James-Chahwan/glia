from repo import AuditLog, UserRepo


def make():
    return UserRepo()


class UserService:
    audit: AuditLog

    def __init__(self, repo: UserRepo, cache):
        self.repo = repo
        self.cache = cache
        self.log = AuditLog()
        self.other: UserRepo = make()

    def get(self, uid):
        self.audit.write(uid)
        self.cache.find(uid)
        return self.repo.find(uid)

    def log_it(self, uid):
        return self.log.write(uid)

    def other_find(self, uid):
        return self.other.find(uid)
