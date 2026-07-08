"""Flask route -> view handler (HANDLED_BY)."""
from flask import Flask

app = Flask(__name__)


@app.route("/users")
def list_users():
    return {"users": []}


@app.route("/users/<int:uid>", methods=["GET"])
def get_user(uid):
    return {"id": uid}
