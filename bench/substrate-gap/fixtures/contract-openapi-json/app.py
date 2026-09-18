"""Flask implementation of the operation declared in openapi.json."""
from flask import Flask

app = Flask(__name__)


@app.route("/users")
def list_users():
    return {"users": []}
