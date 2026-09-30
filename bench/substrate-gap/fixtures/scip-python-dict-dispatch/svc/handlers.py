from svc.repos import get_repo


def handle(row):
    repo = get_repo("users")
    return repo.save(row)
