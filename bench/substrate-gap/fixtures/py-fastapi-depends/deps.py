"""Dependency providers wired into the FastAPI handlers in app.py."""


class Database:
    def find(self, uid):
        return {"id": uid}


def get_db():
    return Database()


def verify_token():
    return True
