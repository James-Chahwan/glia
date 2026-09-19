from flask import Flask

app = Flask(__name__)


@app.route("/invoices", methods=["POST"])
def create_invoice():
    return {}
