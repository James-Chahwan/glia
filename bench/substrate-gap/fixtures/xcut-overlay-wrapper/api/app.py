from flask import Flask

app = Flask(__name__)


@app.route("/users", methods=["GET"])
def list_users():
    return []


@app.route("/users", methods=["POST"])
def create_user():
    return {}


@app.route("/orders", methods=["GET"])
def list_orders():
    return []
