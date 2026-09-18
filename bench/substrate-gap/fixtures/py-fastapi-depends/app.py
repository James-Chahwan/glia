"""FastAPI handlers taking their dependencies through Depends(), in both
spellings: a parameter default and an Annotated[...] marker."""
from typing import Annotated

from fastapi import Depends, FastAPI

from deps import get_db, verify_token, Database

app = FastAPI()


@app.get("/users/{uid}")
def read_user(uid: int, db = Depends(get_db)):
    return db.find(uid)


@app.get("/admin")
def admin(ok: Annotated[bool, Depends(verify_token)]):
    return ok
