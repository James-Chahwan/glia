from flask import Flask

app = Flask(__name__)


@app.route("/orders")
def list_orders():
    return []
