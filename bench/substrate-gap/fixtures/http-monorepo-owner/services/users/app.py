from flask import Flask

app = Flask(__name__)


@app.route("/health")
def users_health():
    return {"service": "users", "ok": True}
