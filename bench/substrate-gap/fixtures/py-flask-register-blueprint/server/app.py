from flask import Flask

from svc.items import bp
from svc import orders

app = Flask(__name__)
app.register_blueprint(bp, url_prefix="/svc")
app.register_blueprint(orders.orders)


@app.route("/health")
def health():
    return "ok"
