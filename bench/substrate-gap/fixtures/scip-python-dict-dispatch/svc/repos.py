class UserRepo:
    def save(self, row):
        return row


class OrderRepo:
    def save(self, row):
        return row


REPOS = {"users": UserRepo()}


def get_repo(name):
    return REPOS[name]
