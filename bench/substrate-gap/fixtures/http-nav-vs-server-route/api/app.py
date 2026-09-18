"""Backend serving the same path the SPA navigates to."""
from flask import Flask

app = Flask(__name__)


@app.route('/users')
def list_users():
    return []
