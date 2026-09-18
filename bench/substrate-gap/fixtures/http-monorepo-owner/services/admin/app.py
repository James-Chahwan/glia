from flask import Flask

app = Flask(__name__)


@app.route("/health")
def admin_health():
    return {"service": "admin", "ok": True}
