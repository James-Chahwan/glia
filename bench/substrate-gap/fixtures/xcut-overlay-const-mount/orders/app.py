from flask import Flask

app = Flask(__name__)


@app.route("/users", methods=["GET"])
def orders_users():
    return []


@app.route("/orders", methods=["GET"])
def orders_list():
    return []
