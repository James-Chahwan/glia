from flask import Flask

app = Flask(__name__)


@app.route("/users", methods=["GET"])
def billing_users():
    return []
