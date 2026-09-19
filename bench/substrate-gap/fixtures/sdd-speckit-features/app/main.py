"""Flask service implementing the spec-kit features under specs/."""
from flask import Flask

app = Flask(__name__)


@app.get("/orders")
def list_orders():
    return {"orders": []}


@app.get("/health")
def health():
    return {"ok": True}
