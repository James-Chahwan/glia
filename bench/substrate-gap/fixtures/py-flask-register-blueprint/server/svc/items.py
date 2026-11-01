from flask import Blueprint, jsonify

bp = Blueprint("items", __name__)


@bp.route("/items")
def list_items():
    return jsonify([])


@bp.route("/items/<int:item_id>", methods=["DELETE"])
def delete_item(item_id):
    return "", 204
