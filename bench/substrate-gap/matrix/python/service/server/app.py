from dataclasses import dataclass

from fastapi import FastAPI

app = FastAPI()


@dataclass
class UserService:
    prefix: str = "u"

    def all(self):
        return []

    def count(self):
        return 0


@app.get("/users")
def list_users():
    return UserService().all()
