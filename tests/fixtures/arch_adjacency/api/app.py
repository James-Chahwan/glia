# The `api` service, in Flask. Its ROUTE nodes carry a bare-text ROUTE_METHOD
# cell ("GET") with no file, so `glia arch` can only place them through the
# route's HANDLED_BY edge to list_users / create_user.
from flask import Flask

app = Flask(__name__)


@app.route("/users", methods=["GET"])
def list_users():
    return []


@app.route("/users", methods=["POST"])
def create_user():
    return {}, 201
