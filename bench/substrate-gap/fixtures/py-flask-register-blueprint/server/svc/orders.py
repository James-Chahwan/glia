from flask import Blueprint, jsonify

orders = Blueprint("orders", __name__, url_prefix="/orders")


@orders.get("/recent")
def recent_orders():
    return jsonify([])
