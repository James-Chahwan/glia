"""Flask provider verified by the web consumer's pact.json."""
from flask import Flask

app = Flask(__name__)


@app.route("/users")
def list_users():
    return {"users": []}
