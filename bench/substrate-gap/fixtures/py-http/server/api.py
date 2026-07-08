"""Flask server exposing the routes the client calls."""
from flask import Flask

app = Flask(__name__)


@app.route("/users", methods=["GET", "POST"])
def users():
    return {"users": []}


@app.route("/users/<int:uid>")
def get_user(uid):
    return {"id": uid}
