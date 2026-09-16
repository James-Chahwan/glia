"""Flask implementation of the operations declared in openapi.yaml."""
from flask import Flask

app = Flask(__name__)


@app.route("/users")
def list_users():
    return {"users": []}


@app.route("/users", methods=["POST"])
def create_user():
    return {"id": 1}, 201
